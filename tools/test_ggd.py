#!/usr/bin/env python3
"""Self-contained checks for tools/ggd.py.  No game data needed.

    python3 tools/test_ggd.py
"""

import os
import struct
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import ggd


def px(r, g, b, a=0):
    return bytes((b, g, r, a))


def gga_header(w, h, flags=0x11200000, payload=b""):
    hdr = bytearray(b"GGA00000")
    hdr += struct.pack("<HHIIIIII", w, h, flags, 0x30, len(payload), 0x30, 0, 0)
    hdr += bytes(0x30 - len(hdr))
    assert len(hdr) == 0x30
    return bytes(hdr) + payload


def test_gga_each_opcode():
    # 4x2 image; every opcode appears at least once.
    p = bytearray()
    p += bytes([0x0C]) + px(1, 2, 3, 0)          # 0C raw run of 1
    p += bytes([0x00, 2])                        # 00 repeat prev x2
    p += bytes([0x0D]) + px(4, 5, 6, 1) + px(7, 8, 9, 2)   # 0D raw run of 2
    p += bytes([0x09])                           # 09 from row above (pixel 0)
    p += bytes([0x0A])                           # 0A from w+1
    p += bytes([0x0B])                           # 0B from w-1
    p += bytes([0x08])                           # 08 repeat prev
    data = gga_header(4, 2, payload=bytes(p))
    w, h, out = ggd.decode_gga(data)
    assert (w, h) == (4, 2)
    got = [out[i * 4:i * 4 + 4] for i in range(8)]
    assert got[0] == px(1, 2, 3, 0), got[0]
    assert got[1] == got[2] == got[0]
    assert got[3] == px(4, 5, 6, 1)
    assert got[4] == px(7, 8, 9, 2)
    # row 2: 09 -> pixel 0, 0A -> pixel 1 of row 1 (w+1 back from index 5 = 0)
    assert got[4] == px(7, 8, 9, 2)
    assert got[5] == got[0], (got[5], got[0])
    assert got[6] == got[1], (got[6], got[1])
    assert got[7] == got[4], (got[7], got[4])


def test_gga_overlap_replicates():
    # 04 <d8> <k8> with d < k must self-replicate, like the C loop.
    p = bytes([0x0C]) + px(10, 20, 30)
    p += bytes([0x0D]) + px(40, 50, 60) + px(70, 80, 90)
    p += bytes([0x04, 2, 4])   # copy 4 px from distance 2 -> ABAB
    data = gga_header(7, 1, payload=p)
    _w, _h, out = ggd.decode_gga(data)
    got = [out[i * 4:i * 4 + 4] for i in range(7)]
    assert got[3:7] == [got[1], got[2], got[1], got[2]], got


def test_gga_16bit_forms():
    p = bytes([0x0C]) + px(1, 1, 1)
    p += bytes([0x01, 0x03, 0x00])                 # repeat x3 (16-bit count)
    p += bytes([0x0C]) + px(2, 2, 2)
    p += bytes([0x03, 0x05, 0x00])                 # copy 1 from d=5
    data = gga_header(5, 1, payload=p)
    _w, _h, out = ggd.decode_gga(data)
    got = [out[i * 4:i * 4 + 4] for i in range(5)]
    assert got[1] == got[2] == got[3] == px(1, 1, 1)
    assert got[4] == px(2, 2, 2), got[4]


def test_ggd_roundtrip():
    w, h = 37, 5
    src = bytearray()
    for y in range(h):
        for x in range(w):
            src += bytes(((x * 7) & 0xFF, (y * 11) & 0xFF, (x + y) & 0xFF))
    blob = ggd.encode_raw(w, h, bytes(src))
    gw, gh, out = ggd.decode_ggd(blob)
    assert (gw, gh) == (w, h)
    assert out == bytes(src)


def test_ggd_compressed_forms():
    # 00 repeat, 01 copy, 03 copy-one, 05 raw, on a 6x1 image.
    s = bytearray([0xB9, 0xAA, 0xB3, 0xB3])
    s += struct.pack("<HH", 6, 1)
    s += bytes([0x05]) + bytes((1, 2, 3))      # raw 1 px
    s += bytes([0x00, 2])                      # repeat x2
    s += bytes([0x06]) + bytes((4, 5, 6, 7, 8, 9))  # raw 2 px
    s += bytes([0x03, 1])                      # copy 1 px from d=1
    s += bytes([0x03, 2])                      # copy 1 px from d=2
    w, h, out = ggd.decode_ggd(bytes(s))
    assert (w, h) == (6, 1)
    assert out == bytes((1, 2, 3, 1, 2, 3, 1, 2, 3, 4, 5, 6, 7, 8, 9, 7, 8, 9))


def test_parse_header():
    assert ggd.parse_header(bytes([0xB9, 0xAA, 0xB3, 0xB3, 4, 0, 2, 0]) + bytes(8)) == (4, 2, 8, "GGD")
    d = gga_header(9, 7, payload=b"")
    assert ggd.parse_header(d) == (9, 7, 0x30, "GGA")
    assert ggd.gga_has_alpha(gga_header(1, 1, flags=0x11200000)) is True
    assert ggd.gga_has_alpha(gga_header(1, 1, flags=0x10200000)) is False


def main():
    tests = [v for k, v in sorted(globals().items()) if k.startswith("test_")]
    bad = 0
    for t in tests:
        try:
            t()
            print("ok   %s" % t.__name__)
        except Exception as e:
            bad += 1
            print("FAIL %s: %r" % (t.__name__, e))
    print("%d/%d passed" % (len(tests) - bad, len(tests)))
    return 1 if bad else 0


if __name__ == "__main__":
    raise SystemExit(main())
