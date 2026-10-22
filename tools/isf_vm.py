#!/usr/bin/env python3
"""一个最小可跑的 ISF 虚拟机（Python 参考实现）。

    python3 tools/isf_vm.py <KIZUNAR.EXE> <ISF归档> <文件.ISF> [--trace] [--steps N]

只实现已经反编译确认过语义的指令：

    0x00 NOP      0x03 END      0x04 GOTO     0x05 CALL     0x06 RET
    0x07 SWITCH   0x08 CSWITCH  0x2B TEXT     0x3A SYSIF    0x44 ASSIGN
    0x47 IF       0x85 INPUT    0xF4 SET_FLAG 0xF5 SET_FLAG2
    0xFB TMRST    0xFC TMR

其余指令按长度跳过（不会中断执行），所以能完整走完一个脚本；其中 0x02/0x56/0x78/0xF0
的载荷是纯 ASCII，会被记进事件流，用来还原「哪张图、哪句语音、哪句台词」的顺序。

寄存器约定见 docs/04-isf-vm.md。
"""
import os
import random
import struct
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from isf_crypto import find_secret, load_archive, decrypt_isf
from isf_text import decode_text

MAX_VARS = 0x10000
MAX_RET = 0x400

NAMES = {
    0x00: "NOP", 0x02: "CMDNAME", 0x03: "END", 0x04: "GOTO", 0x05: "CALL",
    0x06: "RET", 0x07: "SWITCH", 0x08: "CSWITCH", 0x2B: "TEXT", 0x3A: "SYSIF",
    0x3B: "SLOT", 0x44: "ASSIGN", 0x47: "IF", 0x56: "IMAGE", 0x57: "PLACE",
    0x78: "VOICE", 0x85: "INPUT", 0xF0: "VOICE_ARC", 0xF4: "SET_FLAG",
    0xF5: "SET_FLAG2", 0xFB: "TMRST", 0xFC: "TMR",
}

ARITH = {0: "+", 1: "-", 2: "*", 3: "/", 4: "%"}
CMP = {0: "==", 1: "<", 2: "<=", 3: ">", 4: ">=", 5: "!="}


class Halt(Exception):
    pass


def cdiv(a, b):
    """C 的整数除法：向零截断。"""
    if b == 0:
        return 0
    q = abs(a) // abs(b)
    return q if (a < 0) == (b < 0) else -q


def cmod(a, b):
    """C 的取余：符号跟被除数。"""
    if b == 0:
        return 0
    return a - b * cdiv(a, b)


def ascii_of(payload):
    return "".join(chr(b) if 32 <= b < 127 else "" for b in payload).strip("\x00")


class VM:
    def __init__(self, plain, head_len, labels, steps=500000, trace=False):
        self.plain = plain
        self.base = head_len
        self.labels = labels
        self.code = head_len
        self.vars = [0] * MAX_VARS
        self.ret = []
        self.flags = {}
        self.events = []
        self.texts = []
        self.steps = 0
        self.limit = steps
        self.trace = trace
        self.voice = None
        self.unknown = {}
        self.sys = {}
        self.headless = True
        # 0xFB/0xFC 用的虚拟时钟（毫秒）。真引擎读 timeGetTime()，这里每次轮询 +16ms，
        # 相当于 60fps 一帧一次，等待循环因此能在有限步数内走完。
        self.clock = 0
        # 0x85 的宿主回调：poll_input(mode, flags) -> (hotspot, event)
        self.poll_input = None

    # ---- 取指原语（对应 FUN_00434960/80/90）----
    def u8(self):
        v = self.plain[self.code]
        self.code += 1
        return v

    def u16(self):
        v = struct.unpack_from("<H", self.plain, self.code)[0]
        self.code += 2
        return v

    def u32raw(self):
        v = struct.unpack_from("<I", self.plain, self.code)[0]
        self.code += 4
        return v

    # ---- 操作数解码（对应 FUN_00424b20）----
    def operand(self, raw):
        v = raw & 0x3FFFFFFF
        if raw & 0x20000000:
            v -= 0x40000000
        if raw & 0x80000000:
            v = self.vars[v] if 0 <= v < MAX_VARS else 0
        if (raw & 0x40000000) and v:
            q = abs(v)
            v = random.randrange(q) if v > 0 else -random.randrange(q)
        return v

    def val(self):
        return self.operand(self.u32raw())

    def jump_label(self, idx):
        if not (0 <= idx < len(self.labels)):
            raise Halt("label index %d out of range (%d labels)" % (idx, len(self.labels)))
        self.code = self.base + self.labels[idx]

    def set_var(self, idx, v):
        if 0 <= idx < MAX_VARS:
            self.vars[idx] = v

    # ---- 主循环 ----
    def run(self):
        while True:
            if self.steps >= self.limit:
                raise Halt("step limit %d reached" % self.limit)
            if not (self.base <= self.code < len(self.plain)):
                raise Halt("code pointer 0x%x out of range" % self.code)
            start = self.code
            op = self.u8()
            ln = self.u8()
            head = 2
            if ln & 0x80:
                ln = ((ln << 8) | self.u8()) & 0x7FFF
                head = 3
            end = start + ln
            if ln < head or end > len(self.plain):
                raise Halt("bad length %d at 0x%x" % (ln, start))
            self.steps += 1
            if self.trace:
                print("  %6d  0x%06x  0x%02X %-9s len=%3d  %s"
                      % (self.steps, start, op, NAMES.get(op, "?"), ln,
                         self.plain[self.code:end].hex(" ")[:44]))
            jumped = self.exec_op(op, end)
            if not jumped:
                self.code = end

    def do_input(self, mode, flags):
        """0x85：轮询鼠标/键盘。返回 (热点 id, 事件)。

        真引擎里（FUN_00440b40）：hotspot = 命中的 UI 热区序号，没有就 -1，
        鼠标在画面外是 -2；event = 0 无、2 确定、-1 取消。
        没人操作时热点一直是 -1、事件一直是 0，所以菜单脚本会原地空转——
        这是设计如此，不是 VM 的 bug。
        """
        if self.poll_input is not None:
            return self.poll_input(mode, flags)
        return (-1, 0)

    def sys_test(self, sid, arg):
        """0x3A：按引擎系统状态跳转（FUN_004383d0）。未模拟的状态一律返回 False。"""
        g = self.sys
        if sid == 0x00:
            return g.get(0xAC, 0) == arg
        if sid == 0x01:
            return False                      # 鼠标/输入模式，需要真实输入才有意义
        if sid in (0x02, 0x03, 0x04, 0x05, 0x06, 0x07):
            key = {0x02: 0xC8, 0x03: 0xD8, 0x04: 0xDC, 0x05: 0xE0,
                   0x06: 0xCC, 0x07: 0xD0}[sid]
            return g.get(key, 0) == (1 if arg == 0 else 0)
        if sid == 0x0A:
            return arg != 0
        if sid == 0x0B:
            return (g.get(0xA1, 0) != 0) == (arg != 0)
        if sid == 0x0E:
            # FUN_00435d50：输入状态位（+0x54D | 2）。无人值守时当作「已点击」。
            return (3 if self.headless else 0) == arg
        if sid == 0x0C:
            return (g.get(0x54A, 0) == 1) == (arg == 0)
        return False

    def exec_op(self, op, end):
        if op == 0x00:
            return False
        if op == 0x05:
            idx = self.u16()
            if len(self.ret) >= MAX_RET:
                raise Halt("return stack overflow")
            self.ret.append(self.code - self.base)
            self.jump_label(idx)
            return True
        if op == 0x06:
            if not self.ret:
                raise Halt("return stack underflow")
            self.code = self.base + self.ret.pop()
            return True
        if op == 0x07:
            v = self.val()
            n = self.u8()
            if 0 <= v < n:
                self.code += v * 2
                self.jump_label(self.u16())
                return True
            return False
        if op == 0x44:
            dst = self.u16()
            acc = 0
            term = 0
            while True:
                o = self.plain[self.code]
                if o > 4:
                    break
                self.code += 1
                v = self.val()
                if (self.plain[self.code] & 0xF) > 1:
                    if o == 1:
                        term = -v
                    elif o == 2:
                        term = term * v
                    elif o == 3:
                        term = cdiv(term, v)
                    elif o == 4:
                        term = cmod(term, v)
                    else:
                        term = v
                else:
                    if o == 0:
                        acc += v
                    elif o == 1:
                        acc -= v
                    elif o == 2:
                        acc += v * term
                        term = 0
                    elif o == 3:
                        acc += cdiv(term, v)
                        term = 0
                    elif o == 4:
                        acc += cmod(term, v)
                        term = 0
            self.set_var(dst, acc + term)
            return False
        if op == 0x47:
            while True:
                a = self.val()
                c = self.u8()
                b = self.val()
                ok = {0: a == b, 1: a < b, 2: a <= b,
                      3: a > b, 4: a >= b, 5: a != b}.get(c, False)
                if not ok:
                    return False
                act = self.u8()
                if act == 0:
                    self.jump_label(self.u16())
                    return True
                if act == 1:
                    slot = self.u16()
                    self.set_var(slot, self.val())
                    return False
                if act == 0xFF:
                    return False
        if op == 0x03:
            # FUN_00436460：结束脚本。CGLIB/INIT/SELOAD/TCG/YANE 用这个收尾。
            raise Halt("END (0x03)")
        if op == 0x04:
            self.jump_label(self.u16())
            return True
        if op == 0x08:
            # FUN_00436660：和 0x07 完全一样的操作数，但跳过去之前压返回地址。
            v = self.val()
            n = self.u8()
            if 0 <= v < n:
                self.code += v * 2
                if len(self.ret) >= MAX_RET:
                    raise Halt("return stack overflow")
                # 返回地址是「本指令末尾」，不是操作数中间（引擎里传的就是 end 指针）
                self.ret.append(end - self.base)
                self.jump_label(self.u16())
                return True
            return False
        if op == 0x85:
            # FUN_0043b800 → FUN_00440b40：把「命中热区」「事件」写进两个变量。
            va = self.u16()
            vb = self.u16()
            mode = self.u8()
            flags = self.u8()
            if not (flags & 2):          # bit1 置位时走 FUN_004407a0，只更新悬停状态
                spot, ev = self.do_input(mode, flags & 1)
                self.set_var(va, spot)
                self.set_var(vb, ev)
            return False
        if op == 0xFB:
            # FUN_004345c0：把计时器清零（引擎 +0x184 = 0）
            self.clock = 0
            return False
        if op == 0xFC:
            # FUN_0043dcb0：vars[u16] = FUN_004345d0() = timeGetTime() 累计毫秒
            self.clock += 16
            self.set_var(self.u16(), self.clock)
            return False
        if op == 0x3A:
            sid = self.u8()
            arg = self.u8()
            idx = self.u16()
            if self.sys_test(sid, arg):
                self.jump_label(idx)
                return True
            return False
        if op == 0x2B:
            payload = self.plain[self.code:end]
            spk, txt = decode_text(payload)
            self.texts.append((spk, txt, self.voice))
            self.events.append(("TEXT", spk, txt[:24], self.voice))
            self.voice = None
            return False
        if op == 0x78:
            self.voice = ascii_of(self.plain[self.code:end])
            return False
        if op == 0xF4:
            self.flags[0x7FD] = self.u8() != 0
            return False
        if op == 0xF5:
            v = self.u8()
            self.flags[0x83D] = v != 0
            if v == 1:
                self.flags[0x83C] = True
            return False
        if op in (0x02, 0x56, 0xF0):
            s = ascii_of(self.plain[self.code:end])
            self.events.append((NAMES[op], s))
            return False
        self.unknown[op] = self.unknown.get(op, 0) + 1
        return False


def load(exe, arc_path, want):
    secret = find_secret(open(exe, "rb").read())
    arc = load_archive(arc_path)
    key = next((k for k in arc if k.upper() == want.upper()), None)
    if key is None:
        raise SystemExit("no member %r" % want)
    plain, head_len, ops = decrypt_isf(arc[key], secret)
    n = (head_len - 8) // 4
    labels = list(struct.unpack_from("<%dI" % n, plain, 8))
    return key, plain, head_len, labels


def main(argv):
    if len(argv) < 4:
        print(__doc__.strip())
        return 2
    trace = "--trace" in argv
    steps = 500000
    if "--steps" in argv:
        steps = int(argv[argv.index("--steps") + 1])
    out = None
    if "--out" in argv:
        out = argv[argv.index("--out") + 1]
    key, plain, head_len, labels = load(argv[1], argv[2], argv[3])
    vm = VM(plain, head_len, labels, steps=steps, trace=trace)
    print("%s  code 0x%x..0x%x  labels=%d" % (key, head_len, len(plain), len(labels)))
    try:
        vm.run()
    except Halt as e:
        print("HALT: %s" % e)
    print("steps=%d  texts=%d  ret_depth=%d  flags=%s"
          % (vm.steps, len(vm.texts), len(vm.ret), vm.flags))
    if vm.unknown:
        print("skipped opcodes: %s"
              % ", ".join("0x%02X x%d" % (k, v) for k, v in sorted(vm.unknown.items())))
    if out:
        with open(out, "w", encoding="utf-8") as f:
            for spk, txt, voice in vm.texts:
                tag = "[%s] " % voice if voice else ""
                who = "S%d: " % spk if spk is not None else ""
                f.write(tag + who + txt.replace(chr(10), "\\n") + chr(10))
        print("wrote %s (%d lines)" % (out, len(vm.texts)))
    for e in vm.events[:15]:
        print("   ", e)
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
