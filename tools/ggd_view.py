#!/usr/bin/env python3
"""把未压缩的 GGD 渲染成 PNG。

用法：
    python3 ggd_view.py <in.ggd> <out.png>
    python3 ggd_view.py <SM2MPX10 归档> <归档内文件名> <out.png>

像素格式（已实测）：
    0x00  4   魔数 b9 aa b3 b3
    0x04  u16 宽
    0x06  u16 高
    0x08  ... 像素，24bpp **BGR**，行字节数 = (文件大小 - 8) / 高

行尾可能有 0/1/2 字节填充，所以不要用 3*宽 当步长，要用 (size-8)//h。
压缩流（原版 GGD 0x08 处为 0x05）暂不支持。
"""
import os
import struct
import sys
import zlib

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from isf_crypto import load_archive


def write_png(path, w, h, rgb):
    raw = b"".join(b"\x00" + rgb[i * w * 3:(i + 1) * w * 3] for i in range(h))

    def chunk(tag, data):
        body = tag + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)

    out = b"\x89PNG\r\n\x1a\n"
    out += chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
    out += chunk(b"IDAT", zlib.compress(raw, 6))
    out += chunk(b"IEND", b"")
    with open(path, "wb") as f:
        f.write(out)


def decode(data):
    """返回 (w, h, RGB 字节)。"""
    if data[:4] != b"\xb9\xaa\xb3\xb3":
        raise ValueError("不是 GGD")
    w, h = struct.unpack_from("<HH", data, 4)
    stride = (len(data) - 8) // h
    if stride < w * 3:
        raise ValueError("压缩流或数据不足（stride=%d, 需要 %d）" % (stride, w * 3))
    src = data[8:]
    out = bytearray()
    for i in range(h):
        row = src[i * stride:i * stride + w * 3]
        for j in range(0, len(row) - 2, 3):
            out += bytes((row[j + 2], row[j + 1], row[j]))
    return w, h, bytes(out)


def main(argv):
    if len(argv) == 3:
        with open(argv[1], "rb") as f:
            data = f.read()
        out = argv[2]
    elif len(argv) == 4:
        data = load_archive(argv[1])[argv[2]]
        out = argv[3]
    else:
        print(__doc__)
        return 1
    w, h, rgb = decode(data)
    write_png(out, w, h, rgb)
    print("%dx%d -> %s" % (w, h, out))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
