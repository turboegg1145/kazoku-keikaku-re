#!/usr/bin/env python3
"""SM2MPX10 归档读写（GDT 引擎通用容器）。

布局（小端）：
    0x00  8   魔数 "SM2MPX10"
    0x08  u32 条目数
    0x0C  u32 目录区字节数（本作用不到，目录固定从 0x20 开始）
    0x10  12  归档内部名（NUL 填充）
    0x1C  u32 常量 0x20
    0x20  ... 目录项，每项 20 字节：
              0x00 12  文件名（cp932，NUL 填充）
              0x0C u32 数据偏移
              0x10 u32 数据长度

用法：
    python3 sm2mpx.py list   <归档>
    python3 sm2mpx.py extract <归档> <输出目录> [文件名过滤子串]
"""
import os
import struct
import sys

MAGIC = b"SM2MPX10"
HEADER_SIZE = 0x20
ENTRY_SIZE = 20


def parse(path):
    """返回 (原始数据, [(名字, 偏移, 长度), ...])。"""
    with open(path, "rb") as f:
        data = f.read()
    if data[:8] != MAGIC:
        raise ValueError("不是 SM2MPX10 归档: %r" % data[:8])
    count = struct.unpack_from("<I", data, 8)[0]
    entries = []
    for i in range(count):
        base = HEADER_SIZE + i * ENTRY_SIZE
        name = data[base:base + 12].split(b"\0")[0].decode("cp932", "replace")
        offset, size = struct.unpack_from("<II", data, base + 12)
        if offset + size > len(data):
            raise ValueError("目录项越界: %s @%#x+%#x" % (name, offset, size))
        entries.append((name, offset, size))
    return data, entries


def main(argv):
    if len(argv) < 3:
        print(__doc__)
        return 1
    cmd, path = argv[1], argv[2]
    data, entries = parse(path)
    if cmd == "list":
        for name, offset, size in entries:
            print("%-14s %8d  @%#x" % (name, size, offset))
        print("-- %d 项, 共 %d 字节" % (len(entries), len(data)))
        return 0
    if cmd == "extract":
        outdir = argv[3]
        needle = argv[4].upper() if len(argv) > 4 else ""
        os.makedirs(outdir, exist_ok=True)
        for name, offset, size in entries:
            if needle and needle not in name.upper():
                continue
            with open(os.path.join(outdir, name), "wb") as f:
                f.write(data[offset:offset + size])
        return 0
    print(__doc__)
    return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv))
