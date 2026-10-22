#!/usr/bin/env python3
"""ISF 反汇编器。

    python3 tools/isf_dis.py <KIZUNAR.EXE> <ISF归档> <文件.ISF> [起始序号] [条数]

输出每条指令的序号、文件内偏移、opcode、助记符、载荷。语义表见 docs/04-isf-vm.md。
"""
import sys
import os

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from isf_crypto import find_secret, load_archive, decrypt_isf

NAMES = {
    0x00: "NOP",
    0x02: "CMDNAME",
    0x05: "CALL",
    0x06: "RET",
    0x07: "SWITCH",
    0x2B: "TEXT",
    0x3B: "SLOT",
    0x44: "ASSIGN",
    0x47: "IF",
    0x56: "IMAGE",
    0x57: "PLACE",
    0x78: "VOICE",
    0xF0: "VOICE_ARC",
    0xF4: "SET_FLAG",
    0xF5: "SET_FLAG2",
}


def show(pay):
    """十六进制 + 可打印 ASCII 预览。"""
    hx = pay.hex(" ")
    if len(hx) > 54:
        hx = hx[:51] + "..."
    txt = "".join(chr(b) if 32 <= b < 127 else "." for b in pay[:26])
    return "%-54s |%s|" % (hx, txt)


def main(argv):
    if len(argv) < 4:
        print(__doc__.strip())
        return 2
    exe, arc_path, want = argv[1], argv[2], argv[3].upper()
    start = int(argv[4]) if len(argv) > 4 else 0
    count = int(argv[5]) if len(argv) > 5 else 60
    secret = find_secret(open(exe, "rb").read())
    arc = load_archive(arc_path)
    key = next((k for k in arc if k.upper() == want), None)
    if key is None:
        print("no member %r" % want)
        return 1
    plain, head_len, ops = decrypt_isf(arc[key], secret)
    if ops is None:
        print("failed to split instruction stream")
        return 1
    print("%s  head_len=%d  %d instructions" % (key, head_len, len(ops)))
    off = head_len
    for i, (op, pay) in enumerate(ops):
        if i < start:
            off += len(pay) + (3 if len(pay) + 2 >= 0x80 else 2)
            continue
        if i >= start + count:
            break
        name = NAMES.get(op, "?")
        print("  %5d  %06x  0x%02X %-9s %s" % (i, off, op, name, show(pay)))
        off += len(pay) + (3 if len(pay) + 2 >= 0x80 else 2)
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
