"""GGD image codec (Ikura GDL / "GDT" engine).

Reverse-engineered from KIZUNAR.EXE.  The real decoder is FUN_0040b190
(0x40b190..0x40b396); the opcode table below is transcribed from its
decompilation, not guessed.

Two header families occur in the archives:

  family A ("GGD")   magic b9 aa b3 b3   w:u16@4  h:u16@6   stream @ 8
  family B ("GGA")   magic 'GGA00000'    w:u16@8  h:u16@0xA  stream @ 0x0C

The stream is a flat opcode list; there are no per-row offsets and no
"last distance" state.  Decoding stops as soon as w*h pixels exist.

Opcode table (b = next byte)

  00 <k>              repeat the previous pixel k times        (2 bytes)
  01 <k> <d8>         copy k pixels from distance d            (3 bytes)
  02 <k> <d16>        copy k pixels from distance d (16-bit)   (4 bytes)
  03 <d8>             copy 1 pixel  from distance d            (2 bytes)
  04 <d16>            copy 1 pixel  from distance d (16-bit)   (3 bytes)
  05..FF              raw run of (b - 4) pixels                (1 + 3*(b-4) bytes)

Pixels are 3 bytes in B,G,R order.  Distance is measured in pixels, so the
source offset is (pos - d) * 3.  d may cross row boundaries, which is why
02/04 exist: a 640-pixel row is 640 pixels back and needs 16 bits.

Note 05..FF covers 1..251 raw pixels in one byte; 0xFF therefore means
"251 raw pixels", which is exactly the chunking the uncompressed
translation patches use.
"""

import struct
import zlib

MAGIC_A = bytes([0xB9, 0xAA, 0xB3, 0xB3])
MAGIC_B = b"GGA00000"


class GGDError(Exception):
    pass


def parse_header(data):
    """Return (width, height, stream_offset, family) or raise GGDError."""
    if data[:4] == MAGIC_A:
        if len(data) < 8:
            raise GGDError("truncated header")
        w, h = struct.unpack_from("<HH", data, 4)
        return w, h, 8, "GGD"
    if data[:8] == MAGIC_B:
        if len(data) < 12:
            raise GGDError("truncated header")
        w, h = struct.unpack_from("<HH", data, 8)
        return w, h, 12, "GGA"
    raise GGDError("unknown magic %s" % data[:8].hex(" "))


def decode(data):
    """Decode a GGD image.  Returns (width, height, bgr_bytes)."""
    w, h, p, _family = parse_header(data)
    if w == 0 or h == 0:
        raise GGDError("zero dimension %dx%d" % (w, h))
    n = w * h
    out = bytearray()
    end = n * 3
    size = len(data)
    while len(out) < end:
        if p >= size:
            raise GGDError("stream ended after %d/%d pixels" % (len(out) // 3, n))
        b = data[p]
        p += 1
        x = len(out) // 3
        if b == 0:
            k = data[p]
            p += 1
            if x == 0:
                raise GGDError("repeat with no previous pixel")
            px = out[(x - 1) * 3:(x - 1) * 3 + 3]
            out += px * k
        elif b == 1:
            k = data[p]
            d = data[p + 1]
            p += 2
            if d == 0 or d > x:
                raise GGDError("bad distance %d at pixel %d" % (d, x))
            s = (x - d) * 3
            for i in range(k):
                out += out[s + i * 3:s + i * 3 + 3]
        elif b == 2:
            k = data[p]
            d = data[p + 1] | (data[p + 2] << 8)
            p += 3
            if d == 0 or d > x:
                raise GGDError("bad distance %d at pixel %d" % (d, x))
            s = (x - d) * 3
            for i in range(k):
                out += out[s + i * 3:s + i * 3 + 3]
        elif b == 3:
            d = data[p]
            p += 1
            if d == 0 or d > x:
                raise GGDError("bad distance %d at pixel %d" % (d, x))
            s = (x - d) * 3
            out += out[s:s + 3]
        elif b == 4:
            d = data[p] | (data[p + 1] << 8)
            p += 2
            if d == 0 or d > x:
                raise GGDError("bad distance %d at pixel %d" % (d, x))
            s = (x - d) * 3
            out += out[s:s + 3]
        else:
            k = b - 4
            out += data[p:p + 3 * k]
            p += 3 * k
    return w, h, bytes(out[:end])


def encode_raw(w, h, bgr):
    """Build a family-A GGD holding uncompressed pixel data.

    Mirrors the layout the Chinese patch uses: one raw opcode per 251 pixels,
    the last one carrying the remainder.
    """
    if len(bgr) != w * h * 3:
        raise GGDError("pixel buffer is %d bytes, expected %d" % (len(bgr), w * h * 3))
    out = bytearray(MAGIC_A)
    out += struct.pack("<HH", w, h)
    row = w * 3
    for y in range(h):
        base = y * row
        left = w
        while left:
            k = 251 if left > 251 else left
            out.append(k + 4)
            out += bgr[base:base + k * 3]
            base += k * 3
            left -= k
    return bytes(out)


def write_png(path, w, h, bgr):
    """Write a PNG.  Input is B,G,R; PNG wants R,G,B."""
    raw = bytearray()
    for y in range(h):
        raw.append(0)
        row = bgr[y * w * 3:(y + 1) * w * 3]
        for x in range(w):
            b, g, r = row[x * 3], row[x * 3 + 1], row[x * 3 + 2]
            raw += bytes((r, g, b))

    def chunk(tag, payload):
        c = struct.pack(">I", len(payload)) + tag + payload
        return c + struct.pack(">I", zlib.crc32(tag + payload) & 0xFFFFFFFF)

    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(bytes(raw), 9))
    png += chunk(b"IEND", b"")
    with open(path, "wb") as f:
        f.write(png)


def main(argv):
    import os
    import sys
    if len(argv) < 3:
        print("usage: ggd.py <archive|file> <member-stem> [out.png]")
        print("       ggd.py <file.ggd> - [out.png]")
        return 2
    src, stem = argv[1], argv[2]
    out = argv[3] if len(argv) > 3 else None
    if stem == "-":
        data = open(src, "rb").read()
    else:
        sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
        import sm2mpx
        blob, entries = sm2mpx.parse(src)
        table = {os.path.splitext(n)[0].upper(): (o, s) for n, o, s in entries}
        key = stem.upper()
        if key not in table:
            print("no member %r (%d available)" % (stem, len(table)))
            return 1
        off, size = table[key]
        data = blob[off:off + size]
    w, h, px = decode(data)
    print("%dx%d  %d bytes in -> %d bytes out" % (w, h, len(data), len(px)))
    if out:
        write_png(out, w, h, px)
        print("wrote", out)
    return 0


if __name__ == "__main__":
    import sys
    raise SystemExit(main(sys.argv))
