#!/usr/bin/env python3
"""把 41 个 ISF 脚本全跑一遍，导出每条脚本的事件流。

    python3 tools/isf_dump.py <KIZUNAR.EXE> <ISF归档> <输出目录> [--steps N]

每个脚本一个 .txt，按执行顺序记录台词 / 语音 / 图像 / 命令名，
末尾附一段统计（步数、台词数、停在哪里）。菜单类脚本（TITLE/BGMMODE/CGMODE）
在没人操作时会一直空转，这是设计如此，会在统计里标成 step limit。
"""
import os
import struct
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from isf_crypto import find_secret, load_archive, decrypt_isf
import isf_vm

MENU = {"TITLE.ISF", "BGMMODE.ISF", "CGMODE.ISF", "SEXMODE.ISF"}


def dump_one(name, blob, secret, steps):
    plain, head_len, _ = decrypt_isf(blob, secret)
    n = (head_len - 8) // 4
    labels = list(struct.unpack_from("<%dI" % n, plain, 8)) if n > 0 else []
    vm = isf_vm.VM(plain, head_len, labels, steps=steps)
    try:
        vm.run()
        halt = "ok"
    except isf_vm.Halt as e:
        halt = str(e)
    except Exception as e:                       # noqa: BLE001 - 记录而不是中断整批
        halt = "%s: %s" % (type(e).__name__, e)
    return vm, halt


def main(argv):
    if len(argv) < 4:
        print(__doc__.strip())
        return 2
    steps = 2000000
    if "--steps" in argv:
        steps = int(argv[argv.index("--steps") + 1])
    outdir = argv[3]
    os.makedirs(outdir, exist_ok=True)

    secret = find_secret(open(argv[1], "rb").read())
    arc = load_archive(argv[2])
    names = sorted(k for k in arc if k.upper().endswith(".ISF"))
    total_text = 0
    total_step = 0
    rows = []
    for name in names:
        vm, halt = dump_one(name, arc[name], secret, steps)
        total_text += len(vm.texts)
        total_step += vm.steps
        rows.append((name, halt, vm.steps, len(vm.texts)))
        path = os.path.join(outdir, os.path.splitext(name)[0] + ".txt")
        with open(path, "w", encoding="utf-8") as f:
            for kind, *rest in vm.events:
                if kind == "TEXT":
                    spk, txt, voice = rest
                    who = "S%d" % spk if spk is not None else "-"
                    tag = voice or "-"
                    f.write("[TEXT] %-3s %-10s %s\n" % (who, tag, txt.replace("\n", "\\n")))
                else:
                    f.write("[%s] %s\n" % (kind, rest[0]))
        print("%-14s %-38s steps=%-8d texts=%-5d -> %s"
              % (name, halt, vm.steps, len(vm.texts), os.path.basename(path)))

    print()
    print("%-14s %-38s %8s %7s" % ("file", "halt", "steps", "texts"))
    for name, halt, st, tx in rows:
        print("%-14s %-38s %8d %7d" % (name, halt, st, tx))
    print("TOTAL steps=%d texts=%d" % (total_step, total_text))
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
