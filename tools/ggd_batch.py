"""Batch-extract every decodable image from an SM2MPX10 archive to PNG.

    python3 tools/ggd_batch.py GGD out/            # all members
    python3 tools/ggd_batch.py GGD out/ --only GGD  # skip the GGA00000 family
"""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import ggd
import sm2mpx


def main(argv):
    if len(argv) < 3:
        print(__doc__.strip())
        return 2
    src, outdir = argv[1], argv[2]
    only = None
    if "--only" in argv:
        only = argv[argv.index("--only") + 1].upper()
    os.makedirs(outdir, exist_ok=True)
    blob, entries = sm2mpx.parse(src)
    ok = skipped = failed = 0
    for name, off, size in entries:
        data = blob[off:off + size]
        try:
            _w, _h, _off, family = ggd.parse_header(data)
        except ggd.GGDError:
            failed += 1
            continue
        if only and family != only:
            skipped += 1
            continue
        try:
            w, h, px = ggd.decode(data)
        except ggd.GGDError as e:
            print("FAIL %-14s %s" % (name, e))
            failed += 1
            continue
        dst = os.path.join(outdir, os.path.splitext(name)[0] + ".png")
        ggd.write_png(dst, w, h, px)
        ok += 1
    print("decoded %d, skipped %d, failed %d -> %s" % (ok, skipped, failed, outdir))
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
