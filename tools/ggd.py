"""Ikura GDL / "GDT" image codec: GGD (24bpp) and GG0/GG2 (32bpp).

Both decoders are transcribed from KIZUNAR.EXE, not guessed.

  GGD    decoder FUN_0040b190, loader FUN_0040aee0, dispatcher FUN_0040a7d0
  GG0/2  decoder FUN_0040b8e0 (scalar) / FUN_0040bc90 (MMX), loader FUN_0040b650

Family A -- GGD, 24bpp
------------------------
  header   magic b9 aa b3 b3 | w:u16@4 | h:u16@6 | stream @8
  pixels   3 bytes, B,G,R
  stride   the loader allocates align4(w*3) per row, but the stream itself
           is a flat pixel stream with no row padding.

  opcode table (b = next byte)
    00 <k>          repeat previous pixel k times           2 bytes
    01 <k> <d8>     copy k pixels from distance d           3 bytes
    02 <k> <d16>    copy k pixels from distance d, 16-bit   4 bytes
    03 <d8>         copy 1 pixel from distance d            2 bytes
    04 <d16>        copy 1 pixel from distance d, 16-bit    3 bytes
    05..FF          raw run of (b - 4) pixels               1 + 3*(b-4)
  distance is in pixels; d may cross rows, hence the 16-bit forms.
  0xFF therefore means "251 raw pixels" -- exactly the chunking used by the
uncompressed Chinese translation patches.

Family B -- GG0 / GG2, 32bpp BGRA
---------------------------------
  header   0x00 magic "GGA00000"  0x08 w:u16  0x0A h:u16
           0x0C flags:u32  -- byte@0x0F bit0 = alpha plane present
                             byte@0x0F bit5 = palette follows (not seen)
           0x10 payload offset (always 0x30)
           0x14 payload length
           0x18 palette offset, 0x1C palette bytes, 0x20 unknown
           0x30 payload
  pixels   4 bytes, B,G,R,A  (A is ignored when flags bit0 is clear)

  opcode table (d = distance in pixels, k = count)
    00 <k8>            repeat previous pixel k times        2 bytes
    01 <k16>           repeat previous pixel k times        3 bytes
    02 <d8>            copy 1 pixel from distance d         2 bytes
    03 <d16>           copy 1 pixel from distance d         3 bytes
    04 <d8> <k8>       copy k pixels from distance d        3 bytes
    05 <d8> <k16>      copy k pixels from distance d        4 bytes
    06 <d16> <k8>      copy k pixels from distance d        4 bytes
    07 <d16> <k16>     copy k pixels from distance d        5 bytes
    08                 copy previous pixel                   1 byte
    09                 copy pixel from distance w  (row above) 1 byte
    0A                 copy pixel from distance w+1            1 byte
    0B                 copy pixel from distance w-1            1 byte
    0C..FF             raw run of (b - 0x0B) pixels        1 + 4*(b-0x0B)
  copies advance one pixel at a time, so overlapping copies replicate --
the same behaviour as the C loop.

  The decoder runs until the payload is exhausted and never checks w*h, so
the stream itself decides the pixel count.  791 of 792 members land on
exactly w*h; TITLE.GG0 is one pixel short (padded here).
"""

import struct
import zlib

MAGIC_A = bytes([0xB9, 0xAA, 0xB3, 0xB3])
MAGIC_B = b"GGA00000"

GGA_HEADER = 0x30


class GGDError(Exception):
    pass


def parse_header(data):
    """Return (width, height, payload_offset, family).  family is GGD or GGA."""
    if data[:4] == MAGIC_A:
        if len(data) < 8:
            raise GGDError("truncated header")
        w, h = struct.unpack_from("<HH", data, 4)
        return w, h, 8, "GGD"
    if data[:8] == MAGIC_B:
        if len(data) < GGA_HEADER:
            raise GGDError("truncated header")
        w, h = struct.unpack_from("<HH", data, 8)
        off = struct.unpack_from("<I", data, 0x10)[0]
        if off != GGA_HEADER:
            raise GGDError("unexpected payload offset 0x%x" % off)
        return w, h, off, "GGA"
    raise GGDError("unknown magic %s" % data[:8].hex(" "))


def gga_has_alpha(data):
    """flags bit0 (byte 0x0F) selects the separate 8-bit alpha plane."""
    return bool(data[0x0F] & 1)


def decode_ggd(data):
    """Decode a 24bpp GGD image.  Returns (width, height, BGR bytes)."""
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
            out += out[(x - 1) * 3:(x - 1) * 3 + 3] * k
        elif b == 1:
            k = data[p]
            d = data[p + 1]
            p += 2
            s = _dist(x, d) * 3
            for i in range(k):
                out += out[s + i * 3:s + i * 3 + 3]
        elif b == 2:
            k = data[p]
            d = data[p + 1] | (data[p + 2] << 8)
            p += 3
            s = _dist(x, d) * 3
            for i in range(k):
                out += out[s + i * 3:s + i * 3 + 3]
        elif b == 3:
            d = data[p]
            p += 1
            out += out[_dist(x, d) * 3:][:3]
        elif b == 4:
            d = data[p] | (data[p + 1] << 8)
            p += 2
            out += out[_dist(x, d) * 3:][:3]
        else:
            k = b - 4
            out += data[p:p + 3 * k]
            p += 3 * k
    return w, h, bytes(out[:end])


def _dist(pos, d):
    if d == 0 or d > pos:
        raise GGDError("bad distance %d at pixel %d" % (d, pos))
    return pos - d


def decode_gga(data, w=None, pad=True):
    """Decode a 32bpp GG0/GG2 image.  Returns (width, height, BGRA bytes)."""
    hw, hh, off, _family = parse_header(data)
    if w is None:
        w = hw
    h = hh
    if w == 0 or h == 0:
        raise GGDError("zero dimension %dx%d" % (w, h))
    src = data[off:]
    out = bytearray()
    p = 0
    size = len(src)
    while p < size:
        op = src[p]
        p += 1
        if op == 0:
            _rep(out, 1, src[p])
            p += 1
        elif op == 1:
            _rep(out, 1, src[p] | (src[p + 1] << 8))
            p += 2
        elif op == 2:
            _rep(out, src[p], 1)
            p += 1
        elif op == 3:
            _rep(out, src[p] | (src[p + 1] << 8), 1)
            p += 2
        elif op == 4:
            d = src[p]
            k = src[p + 1]
            p += 2
            _rep(out, d, k)
        elif op == 5:
            d = src[p]
            k = src[p + 1] | (src[p + 2] << 8)
            p += 3
            _rep(out, d, k)
        elif op == 6:
            d = src[p] | (src[p + 1] << 8)
            k = src[p + 2]
            p += 3
            _rep(out, d, k)
        elif op == 7:
            d = src[p] | (src[p + 1] << 8)
            k = src[p + 2] | (src[p + 3] << 8)
            p += 4
            _rep(out, d, k)
        elif op == 8:
            _rep(out, 1, 1)
        elif op == 9:
            _rep(out, w, 1)
        elif op == 10:
            _rep(out, w + 1, 1)
        elif op == 11:
            _rep(out, w - 1, 1)
        else:
            k = op - 0x0B
            out += src[p:p + 4 * k]
            p += 4 * k
    if pad:
        end = w * h * 4
        if len(out) < end:
            if out:
                out += out[-4:] * ((end - len(out)) // 4)
            else:
                out += bytes(end)
        del out[end:]
    return w, h, bytes(out)


def _rep(out, d, k):
    """Copy k pixels from distance d, one at a time (overlap replicates)."""
    pos = len(out) // 4
    if d == 0 or d > pos:
        raise GGDError("bad distance %d at pixel %d" % (d, pos))
    s = (pos - d) * 4
    for _ in range(k):
        out += out[s:s + 4]
        s += 4


def decode(data):
    """Decode either family.  Returns (w, h, pixels, channels)."""
    w, h, _off, family = parse_header(data)
    if family == "GGD":
        w, h, px = decode_ggd(data)
        return w, h, px, 3
    w, h, px = decode_gga(data)
    return w, h, px, 4


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


def write_png(path, w, h, pixels, channels=3):
    """Write a PNG.  Input is B,G,R[,A]; PNG wants R,G,B[,A]."""
    raw = bytearray()
    stride = w * channels
    for y in range(h):
        raw.append(0)
        row = pixels[y * stride:(y + 1) * stride]
        for x in range(w):
            o = x * channels
            if channels == 4:
                raw += bytes((row[o + 2], row[o + 1], row[o], row[o + 3]))
            else:
                raw += bytes((row[o + 2], row[o + 1], row[o]))

    def chunk(tag, payload):
        c = struct.pack(">I", len(payload)) + tag + payload
        return c + struct.pack(">I", zlib.crc32(tag + payload) & 0xFFFFFFFF)

    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6 if channels == 4 else 2, 0, 0, 0))
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
    w, h, px, ch = decode(data)
    print("%dx%d  %d bytes in -> %d bytes out (%dbpp)" % (w, h, len(data), len(px), ch * 8))
    if out:
        write_png(out, w, h, px, ch)
        print("wrote", out)
    return 0


if __name__ == "__main__":
    import sys
    raise SystemExit(main(sys.argv))
