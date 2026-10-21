#!/usr/bin/env python3
"""把未压缩的 GGD（像素流以 0xFF 开头）渲染成 PNG。

用法：
    python3 ggd_view.py <in.ggd> <out.png> [归档路径]

如果第一个参数是 SM2MPX10 归档，则第二个参数是归档内文件名，第三个是输出 PNG。

像素格式：24bpp BGR，行宽 = 头部宽 + 1，数据从 0x08 开始。
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
    """返回 (w, h, RGB 字节)。仅支持 0xFF 未压缩流。"""
    if data[:4] != b"\xb9\xaa\xb3\xb3":
        raise ValueError("不是 GGD")
    w, h = struct.unpack_from("<HH", data, 4)
    if data[8] != 0xFF:
        raise ValueError("压缩流（0x%02X），暂不支持" % data[8])
    stride = w + 1
    src = data[9:]
    out = bytearray()
    for i in range(h):
        row = src[i * stride * 3:(i * stride + w) * 3]
        if len(row) < w * 3:
            row = row + bytes(w * 3 - len(row))
        for j in range(0, len(row), 3):
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
