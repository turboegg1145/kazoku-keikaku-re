#!/usr/bin/env python3
"""tools/sav.py 的单元测试：python3 tools/test_sav.py"""

import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import sav

FAILED = []


def check(name, cond, extra=""):
    if cond:
        print("  ok   %s" % name)
    else:
        print("  FAIL %s %s" % (name, extra))
        FAILED.append(name)


def test_slot_size():
    check("存档格固定 60181 字节", sav.SLOT_SIZE == 60181, sav.SLOT_SIZE)
    check("缩略图 0xE128", sav.THUMB_SIZE == 0xE128, hex(sav.THUMB_SIZE))
    check("位数组 129 字节", sav.BITS_BYTES == 129, sav.BITS_BYTES)


def test_empty():
    sf = sav.SaveFile()
    d = sav.build(sf)
    check("空档只有 120 字节表", len(d) == 120, len(d))
    sf2 = sav.parse(d)
    check("空档无存档格", not any(s.used for s in sf2.slots))
    check("空档无最新", sf2.newest() is None)


def test_slot_roundtrip():
    sf = sav.SaveFile()
    s = sf.slots[4]
    s.set_time((2026, 10, 22, 21, 30, 15, 0, 0))
    s.title = "第三章 家族計画"
    s.vars[0] = 7
    s.vars[541] = -12345
    s.bits = bytes([0b1010_0101]) + bytes(128)
    d = sav.build(sf)
    check("带一格的文件长度", len(d) == 120 + sav.SLOT_SIZE, len(d))
    sf2 = sav.parse(d)
    t = sf2.slots[4]
    check("时间回环", t.time == (2026, 10, 22, 21, 30, 15, 0, 0), t.time)
    check("标题回环", t.title == "第三章 家族計画", t.title)
    check("变量回环", t.vars[0] == 7 and t.vars[541] == -12345)
    check("位数组回环", t.bits[:1] == b"\xa5")
    check("最新存档", sf2.newest() == 4, sf2.newest())
    check("块偏移正确", sav.pack_blocks([b"x" * 3] + [b""] * 29)[:4] == b"\x78\x00\x00\x00")


def test_newest():
    sf = sav.SaveFile()
    sf.slots[0].set_time((2026, 9, 1, 10, 0, 0, 0, 0))
    sf.slots[1].set_time((2026, 10, 2, 9, 0, 0, 0, 0))
    sf.slots[2].set_time((2026, 10, 2, 23, 0, 0, 0, 0))
    check("按日期+时间选最新", sf.newest() == 2, sf.newest())


def test_rle():
    cases = [
        (b"\x00" * 200, 1600),
        (b"\xff" * 100, 800),
        (bytes([0b0101_0101]) * 4, 32),
        (b"", 0),
    ]
    for data, n in cases:
        enc = sav.rle_encode(data, n)
        dec = sav.rle_decode(enc, n)
        check("RLE 回环 n=%d" % n, dec[:len(data)] == data, (enc.hex(), dec.hex()))
    enc = sav.rle_encode(bytes(30001 // 8 + 1), 30001)
    check("30001 位全 0 编成 3 字节", enc == bytes.fromhex("407538"), enc.hex())


def test_thumb():
    rgb = bytes([255, 0, 0]) * (320 * 240)
    th = sav.thumb_from_rgb(rgb, 320, 240)
    check("缩略图长度", len(th) == 0xE128, len(th))
    import struct
    sz, w, h, planes, bpp = struct.unpack_from("<IiiHH", th, 0)
    check("BMP 头", (sz, w, h, planes, bpp) == (40, 160, -120, 1, 24), (sz, w, h, planes, bpp))
    check("像素是 BGR", th[40:43] == b"\x00\x00\xff", th[40:43])


def main():
    for fn in (test_slot_size, test_empty, test_slot_roundtrip, test_newest, test_rle, test_thumb):
        print(fn.__name__)
        fn()
    print()
    if FAILED:
        print("%d 项失败" % len(FAILED))
        return 1
    print("全部通过")
    return 0


if __name__ == "__main__":
    sys.exit(main())