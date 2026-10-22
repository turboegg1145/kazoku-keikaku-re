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


def dump_one(name, blob, secret, steps, modules=None, save=None):
    plain, head_len, _ = decrypt_isf(blob, secret)
    n = (head_len - 8) // 4
    labels = list(struct.unpack_from("<%dI" % n, plain, 8)) if n > 0 else []
    vm = isf_vm.VM(plain, head_len, labels, steps=steps, modules=modules,
                   name=name.rsplit(".", 1)[0].upper())
    if save is not None:
        # 真机存档里的块 27/28 是全局位数组/变量数组的落盘副本，0x4A 要读它们。
        vm.sys27 = bytearray(save.system.get(27, b""))
        vm.sys28 = bytearray(save.system.get(28, b""))
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
    modules = isf_vm.load_modules(argv[1], argv[2])
    names = sorted(k for k in arc if k.upper().endswith(".ISF"))
    only = argv[4].upper() if len(argv) > 4 and not argv[4].startswith("--") else None
    if only:
        names = [n for n in names if n.upper().startswith(only)]
    # 真机存档：给 0x4A 提供块 27/28。默认取 EXE 同目录的 KIZUNAR.SAV。
    save = None
    sav_path = (argv[argv.index("--sav") + 1] if "--sav" in argv
                else os.path.join(os.path.dirname(os.path.abspath(argv[1])), "KIZUNAR.SAV"))
    if os.path.exists(sav_path):
        try:
            import sav as savmod
            save = savmod.parse(open(sav_path, "rb").read())
        except Exception:                        # noqa: BLE001 - 存档坏了就当没有
            save = None
    total_text = 0
    total_step = 0
    rows = []
    for name in names:
        vm, halt = dump_one(name, arc[name], secret, steps, modules, save)
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
                elif kind == "CHOICE":
                    win, idx, x, y, w, h, text = rest
                    f.write("[CHOICE] win=%d idx=%d rect=%d,%d,%d,%d text=%s\n"
                            % (win, idx, x, y, w, h, text))
                elif kind == "CHOICE_OPEN":
                    win, count, flags = rest
                    f.write("[CHOICE_OPEN] win=%d count=%d flags=%d\n" % (win, count, flags))
                elif kind == "CHOICE_CLOSE":
                    f.write("[CHOICE_CLOSE] win=%d\n" % rest[0])
                elif kind == "CHOICE_INPUT":
                    win, value, flags = rest
                    f.write("[CHOICE_INPUT] win=%d value=%d flags=%d\n" % (win, value, flags))
                elif kind == "CHOICE_CLEAR":
                    f.write("[CHOICE_CLEAR]\n")
                elif kind == "CHOICE_SHUFFLE":
                    f.write("[CHOICE_SHUFFLE]\n")
                elif kind == "SE_LOAD":
                    slot, name = rest
                    f.write("[SE_LOAD] slot=%d name=%s\n" % (slot, name))
                elif kind == "SE_PLAY":
                    slot, param = rest
                    f.write("[SE_PLAY] slot=%d param=%d\n" % (slot, param))
                elif kind == "SE_STOP":
                    f.write("[SE_STOP] slot=%d\n" % rest[0])
                elif kind == "VOICE_PARAM":
                    f.write("[VOICE_PARAM] %d\n" % rest[0])
                elif kind == "SE_REPEAT":
                    slot, value = rest
                    f.write("[SE_REPEAT] slot=%d value=%d\n" % (slot, value))
                elif kind == "FLAG_VOICE":
                    which, index = rest
                    f.write("[FLAG_VOICE] which=%d idx=%d\n" % (which, index))
                elif kind == "UI_MODE":
                    f.write("[UI_MODE] %d\n" % rest[0])
                elif kind in ("GSAVE", "GLOAD"):
                    which, start, count, x = rest
                    tag = "GSAVE" if kind == "GSAVE" else "GLOAD"
                    f.write("[%s] which=%d start=%d count=%d x=%d\n"
                            % (tag, which, start, count, x))
                elif kind == "SURF":
                    idx, w, h = rest
                    f.write("[SURF] idx=%d %dx%d\n" % (idx, w, h))
                elif kind == "SURF_POS":
                    idx, x, y = rest
                    f.write("[SURF_POS] idx=%d %d,%d\n" % (idx, x, y))
                elif kind == "WIN_COLOR":
                    surf, r, g, b = rest
                    f.write("[WIN_COLOR] surf=%d rgb=%02x%02x%02x\n" % (surf, r, g, b))
                elif kind == "WIN_FX":
                    value, r, g, b, flag = rest
                    f.write("[WIN_FX] value=%d rgb=%02x%02x%02x flag=%d\n"
                            % (value, r, g, b, flag))
                elif kind == "DRAW":
                    cmd, img, x, y, w, h, dur, dx, dy, extra = rest
                    line = ("[DRAW] cmd=%d img=%d src=%d,%d,%d,%d dur=%d dst=%d,%d"
                            % (cmd, img, x, y, w, h, dur, dx, dy))
                    for v in extra:
                        line += " +%d" % v
                    f.write(line + "\n")
                else:
                    f.write("[%s] %s\n" % (kind, " ".join(str(x) for x in rest)))
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
