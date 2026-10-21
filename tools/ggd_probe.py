#!/usr/bin/env python3
"""GGD / GG2 图像容器探针（只做头部与统计，尚未实现解码）。

用法：
    python3 ggd_probe.py <归档> [文件名...]

已确认的 GGD 头（b9 aa b3 b3）：
    0x00  4   魔数 b9 aa b3 b3
    0x04  u16 宽
    0x06  u16 高
    0x08  u8  位深/格式（实测 0x05）
    0x09  3   未知（各文件不同，疑似校验或调色板种子）
    0x0C  1   0x00
    0x0D  ... 压缩数据

GG2 头（GGA00000）：
    0x00  8   魔数 "GGA00000"
    0x08  u16 宽
    0x0A  u16 高
    0x0C  ... 未知
"""
import sys
import os
import collections

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from isf_crypto import load_archive


def probe(name, data):
    magic = data[:4]
    if magic == b"GGA0":
        w, h = int.from_bytes(data[8:10], "little"), int.from_bytes(data[10:12], "little")
        fmt = "GG2"
    else:
        w, h = int.from_bytes(data[4:6], "little"), int.from_bytes(data[6:8], "little")
        fmt = "GGD"
    hist = collections.Counter(data[13:])
    low = sum(n for b, n in hist.items() if b < 8) / max(1, len(data) - 13)
    print("%-14s %-3s %4dx%-4d %7d B  3B/px=%6d  控制字节占比=%.2f  头=%s"
          % (name, fmt, w, h, len(data), w * h * 3, low, data[:13].hex(" ")))


def main(argv):
    if len(argv) < 2:
        print(__doc__)
        return 1
    arc = load_archive(argv[1])
    wanted = [a.upper() for a in argv[2:]]
    for name in sorted(arc):
        if wanted and not any(w in name.upper() for w in wanted):
            continue
        probe(name, arc[name])
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
