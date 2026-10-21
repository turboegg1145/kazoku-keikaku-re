"""GGD compressed-stream tracer / partial decoder.

Status: the token framing below is verified byte-exact against the uncompressed
patch images in ggd_sc/, but one token family (opcode 0x00) is not fully
understood, so full-image decoding is not yet possible.

Verified token forms (each token is 3 bytes unless noted):

  <op> <a> <b>
    0x00  repeat the previous pixel <a> times        (verified on flat runs)
    0x01  copy 2 pixels from distance <b>            (verified)
    0x02  copy ? pixels from distance <a>            (partially verified)
    0x03  copy 1 pixel  from distance <b>            (verified)
  <op> <b>                                            (2 bytes)
    0x04..0x1f  copy 1 pixel from distance <op>      (verified)
  otherwise: literal pixel, 3 raw bytes.

Each row begins with one control byte whose meaning is unknown; the pixel data
of row y starts at <offset of row> + 1.  Distances are measured in pixels over
the whole decoded buffer (they may reach into previous rows).
"""
import sys

MAGIC = b"\xb9\xaa\xb3\xb3"


def parse_header(data):
    if data[:4] != MAGIC:
        raise ValueError("not a GGD file: %s" % data[:4].hex())
    return int.from_bytes(data[4:6], "little"), int.from_bytes(data[6:8], "little")


def trace(data, max_pixels=4096, short_max=0x20):
    """Walk the token stream, yielding (kind, pos, operands, pixels)."""
    w, h = parse_header(data)
    p = 8
    out = []
    for y in range(h):
        lead = data[p]
        p += 1
        yield ("row", p - 1, (lead,), [])
        x = 0
        while x < w and len(out) < max_pixels:
            c = data[p]
            if c == 0x00:
                n = data[p + 1]
                px = out[-1] if out else bytes(3)
                out += [px] * n
                yield ("rep", p, (n, data[p + 2]), [px] * n)
                p += 3
                x += n
            elif c == 0x01:
                d = data[p + 2]
                px = [out[len(out) - d + k] for k in range(2)]
                out += px
                yield ("cp2", p, (d, data[p + 1]), px)
                p += 3
                x += 2
            elif c == 0x02:
                d = data[p + 1]
                px = [out[len(out) - d + k] for k in range(2)]
                out += px
                yield ("cp2b", p, (d, data[p + 2]), px)
                p += 3
                x += 2
            elif c == 0x03:
                d = data[p + 1]
                px = [out[len(out) - d]]
                out += px
                yield ("cp1", p, (d, data[p + 2]), px)
                p += 3
                x += 1
            elif 4 <= c < short_max:
                px = [out[len(out) - c]]
                out += px
                yield ("cp1s", p, (c, data[p + 1]), px)
                p += 2
                x += 1
            else:
                px = [data[p:p + 3]]
                out += px
                yield ("lit", p, (), px)
                p += 3
                x += 1
    return out


def score(data, patch):
    """Return (rows_exact, first_bad_row) comparing a decode against a patch file."""
    w, h = parse_header(data)
    stride = (len(patch) - 8) // h
    out = []
    rows = 0
    for kind, pos, ops, px in trace(data, max_pixels=w * h):
        if kind == "row":
            if len(out) >= w * rows:
                exp = patch[9 + stride * rows: 9 + stride * rows + w * 3]
                if b"".join(out[rows * w:(rows + 1) * w]) != exp:
                    return rows, rows
            continue
        out += px
    for y in range(h):
        exp = patch[9 + stride * y: 9 + stride * y + w * 3]
        if b"".join(out[y * w:(y + 1) * w]) != exp:
            return y, y
        rows = y + 1
    return h, None


def main(argv):
    import sm2mpx
    if len(argv) < 2:
        print("usage: ggd_stream.py <archive> <member> [patch]")
        return 1
    data, entries = sm2mpx.parse(argv[1])
    name = argv[2]
    for n, off, size in entries:
        if n == name:
            blob = data[off:off + size]
            break
    else:
        print("member not found")
        return 1
    w, h = parse_header(blob)
    print("%s %dx%d %d bytes" % (name, w, h, len(blob)))
    for kind, pos, ops, px in trace(blob, max_pixels=200):
        print("  %-5s @%5d %-14s %s" % (kind, pos, ops, b"".join(px).hex()))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
