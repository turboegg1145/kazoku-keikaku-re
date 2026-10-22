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
    def __init__(self, plain, head_len, labels, steps=500000, trace=False,
                 modules=None, name=None):
        self.plain = plain
        self.base = head_len
        self.labels = labels
        self.code = head_len
        # 模块系统（0x02 CALLMOD / 0x03 END）：名字 -> (plain, head_len, labels)。
        # 真引擎在 0x02 时调 FUN_00434b50 重新读档并解密，这里预先解好直接换。
        self.modules = modules or {}
        self.module = name
        self.mod_stack = []          # [(返回后模块名, 返回偏移)]
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
        # 无人值守时自动点确认（导出剧本用）。真实运行时由宿主提供输入。
        self.auto = True
        # 位数组（引擎 vm+0x554）：0x31 写、0x33/0x3B 读。存「读过/解锁」这类旗标。
        self.bits = bytearray(0x10000)
        self.bits_n = 1024        # 0x30 <u16 n> -> n+1
        self.vars_n = 256         # 0x40 <u16 n> -> n+1
        self.seen = bytearray(0x10000)   # 0xA5：已读文本位图
        self.seen_n = 1024
        # 输入状态字节（引擎 vm+0x549/0x54A/0x54C/0x54D/0x54E），0x3A 会读它们。
        self.b549 = self.b54a = self.b54c = self.b54d = self.b54e = 0
        self.b54b = 0
        # 0x37 的两个环境开关：音频就绪（引擎 +0x81C）与文字绘制模式（DAT_0048940C）
        self.audio_ready = True
        self.text_mode = 0
        # 存档块 27/28：全局位数组 / 变量数组的落盘副本，0x49/0x4A 直接读写。
        self.sys27 = bytearray()
        self.sys28 = bytearray()
        # 0xF1 设的等待时长（引擎 vm+0x7B4）
        self.deadline = 0
        # 0x84 定义的绘制槽（引擎 engine+0xE0，64 项）
        self.slots = [None] * 64

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

    def take_str(self, end):
        """读一个 NUL 结尾的 cp932 串（引擎 FUN_004349E0）。"""
        if self.code >= end:
            return ""
        seg = self.plain[self.code:end]
        nul = seg.index(0) if 0 in seg else len(seg)
        # cp932 里 0xFF 之类会落到私用区（U+F8F3），统一丢掉，和 Rust 侧对齐
        s = bytes(seg[:nul]).decode("cp932", "ignore")
        s = "".join(ch for ch in s
                    if ch != "\ufffd" and not ("\ue000" <= ch <= "\uf8ff"))
        self.code = min(self.code + nul + 1, end)
        return s

    # ---- 存档块 27/28 的全局数组读写（0x49/0x4A）----
    def global_load(self, start, count, which):
        if start < 0 or count <= 0:
            return
        if which == 0:
            if start >= self.bits_n:
                return
            base = start // 8
            for i in range(start, min(start + count, self.bits_n)):
                at = base + (i // 8 - base)
                self.bits[i] = (self.sys27[at] >> (i & 7)) & 1 if at < len(self.sys27) else 0
        elif which == 1:
            if start >= self.vars_n:
                return
            for i in range(start, min(start + count, self.vars_n, MAX_VARS)):
                at = i * 4
                if at + 4 <= len(self.sys28):
                    self.vars[i] = struct.unpack_from("<i", self.sys28, at)[0]
                else:
                    self.vars[i] = 0

    def global_store(self, start, count, which):
        if start < 0 or count <= 0:
            return
        if which == 0:
            if start >= self.bits_n:
                return
            base = start // 8
            end = min(start + count, self.bits_n)
            need = base + (end + 7) // 8 - base
            if len(self.sys27) < need:
                self.sys27.extend(b"\x00" * (need - len(self.sys27)))
            for i in range(start, end):
                at = base + (i // 8 - base)
                if self.bits[i]:
                    self.sys27[at] |= 1 << (i & 7)
                else:
                    self.sys27[at] &= ~(1 << (i & 7)) & 0xFF
        elif which == 1:
            if start >= self.vars_n:
                return
            end = min(start + count, self.vars_n, MAX_VARS)
            need = end * 4
            if len(self.sys28) < need:
                self.sys28.extend(b"\x00" * (need - len(self.sys28)))
            for i in range(start, end):
                struct.pack_into("<i", self.sys28, i * 4, self.vars[i])

    def jump_label(self, idx):
        if not (0 <= idx < len(self.labels)):
            raise Halt("label index %d out of range (%d labels)" % (idx, len(self.labels)))
        self.code = self.base + self.labels[idx]

    def set_var(self, idx, v):
        if 0 <= idx < MAX_VARS:
            self.vars[idx] = v

    def load_module(self, name):
        """切换到另一个 ISF 模块，PC 回到模块开头（对应 FUN_00434b50）。

        真引擎会把标签表重定位成绝对地址并把 PC 指到缓冲区起点，这里等价地
        换 plain/base/labels 三件套、code 归位。变量与位数组是全局的，不清。
        """
        plain, head_len, labels = self.modules[name]
        self.plain = plain
        self.base = head_len
        self.labels = labels
        self.code = head_len
        self.module = name

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
        if self.auto:
            # 无人值守：每次轮询都当作「点了第一个热区」，用来把剧本一路推到结尾。
            return (0, 2)
        return (-1, 0)

    def reset_timer(self):
        """0xFB：宿主也应把计时器清零。"""
        self.clock = 0

    def sys_func(self, kind, arg):
        """0x37：FUN_004378F0 的 20 个分支，和 Rust 侧逐行对应。"""
        limit = {0x00: 3, 0x01: 3, 0x08: 3, 0x0E: 4}
        if 0x02 <= kind <= 0x07 or 0x09 <= kind <= 0x0D or 0x0F <= kind <= 0x11 or kind == 0x13:
            limit.setdefault(kind, 2)
        if kind < 0x14 and arg >= limit.get(kind, 0xFF):
            return
        g = self.sys
        ready = self.audio_ready
        if kind == 0x00:
            if arg == 0:
                g[0xBC] = g[0xC0] = g[0xC4] = 1
            elif arg == 1:
                g[0xBC] = 1
                g[0xC0] = 1
                g[0xC4] = 0
                if ready and g.get(0xB8, 0):
                    if self.text_mode == 0:
                        g[0xD4] = 0
                        g[0xAC] = arg
                        g[0xB8] = 1
                        return
                    if self.text_mode > 3:
                        g[0xAC] = arg
                        g[0xBC] = 1
                        g[0xC0] = 1
                        g[0xC4] = 0
                        return
                g[0xB8] = 0
                g[0xAC] = arg
                g[0xD4] = 1
                return
            elif arg == 2:
                g[0xBC] = g[0xC0] = g[0xC4] = 0
                g[0xAC] = arg
                return
            g[0xAC] = arg
            return
        if kind == 0x01:
            if g.get(0xAC, 0) == 2:
                return
            if arg == 0 and ready:
                g[0xD4] = 0
            else:
                g[0xD4] = 0
                if g.get(0xBC, 0) or g.get(0xC4, 0):
                    g[0xD4] = 1
                if arg == 1:
                    g[0xB8] = 0
                    return
            if ready and (g.get(0xC0, 0) or g.get(0xC4, 0)):
                g[0xB8] = 1
                return
            g[0xB8] = 0
            return
        if kind == 0x02:
            g[0xC8] = 1 if arg == 0 else 0
            return
        if kind == 0x03:
            if ready and arg == 0 and (g.get(0xB8, 0) or g.get(0xAC, 0) != 2):
                g[0xD8] = g[0xCC] = 1
            else:
                g[0xCC] = g[0xD8] = 0
            return
        if kind == 0x04:
            if ready and arg == 0:
                g[0xDC] = g[0xD0] = 1
            else:
                g[0xD0] = g[0xDC] = 0
            return
        if kind == 0x05:
            g[0xE0] = 1 if (ready and arg == 0) else 0
            return
        if kind == 0x06:
            g[0xCC] = 1 if (g.get(0xB8, 0) and arg == 0 and g.get(0xD8, 0)) else 0
            return
        if kind == 0x07:
            g[0xD0] = 1 if (g.get(0xD4, 0) and arg == 0 and g.get(0xDC, 0)) else 0
            return
        if kind == 0x08:
            if arg == 0:
                self.b549 = 1
                self.b54d &= ~2 & 0xFF
                self.events.append(("SE_STOP", 0x20))
            elif arg == 1:
                self.b549 = 0
            elif arg == 2:
                self.b549 = 1
                self.b54d &= ~2 & 0xFF
            return
        if kind == 0x09:
            g[0xE4] = 1 if arg == 0 else 0
            return
        if kind == 0x0A:
            g[0xE8] = 1 if arg != 0 else 0
            return
        if kind == 0x0C:
            self.b54a = arg
            return
        if kind == 0x0D:
            return
        if kind == 0x0E:
            if arg & 2:
                self.b549 = 0
            self.b54d = arg
            return
        if kind == 0x0F:
            g[0x801] = 1 if arg != 0 else 0
            return
        if kind == 0x10:
            self.b54b = 1 if arg != 0 else 0
            return
        if kind == 0x11:
            g[0xEC] = 1 if arg != 0 else 0
            return
        if kind == 0x13:
            g[0x2E5] = 1 if arg != 0 else 0
            return
        if kind == 0x15:
            g[0xED] = 1 if arg != 0 else 0
            return
        if kind == 0x16:
            g[0x2E4] = 1 if arg != 0 else 0
            return
        if kind == 0x17:
            g[0xEE] = arg
            return
    def sys_test(self, sid, arg):
        """0x3A：按引擎系统状态跳转（FUN_004383d0）。未模拟的状态一律返回 False。"""
        g = self.sys
        if sid == 0x00:
            return g.get(0xAC, 0) == arg
        if sid == 0x01:
            # 鼠标按键边沿：0 = 刚按下，1 = 刚松开，2 = 按下中
            d4, b8 = g.get(0xD4, 0), g.get(0xB8, 0)
            if arg == 0:
                return d4 == 0 and b8 != 0
            if arg == 1:
                return d4 != 0 and b8 == 0
            return d4 != 0 and b8 != 0
        if sid in (0x02, 0x03, 0x04, 0x05, 0x06, 0x07):
            key = {0x02: 0xC8, 0x03: 0xD8, 0x04: 0xDC, 0x05: 0xE0,
                   0x06: 0xCC, 0x07: 0xD0}[sid]
            return g.get(key, 0) == (1 if arg == 0 else 0)
        if sid == 0x0A:
            return arg != 0
        if sid == 0x0B:
            return (g.get(0xA1, 0) != 0) == (arg != 0)
        if sid == 0x08:
            # FUN_00435c40(vm,0)：输入被锁（+0x54C）时返回 1，否则返回 +0x549
            held = 1 if self.b54c else self.b549
            return (held != 0) == (arg == 0)
        if sid == 0x0C:
            return (self.b54a == 1) == (arg == 0)
        if sid == 0x0D:
            # FUN_0043f970 → 文字管理器 FUN_00413fe0：8 个消息槽里有没有正在打字的。
            # 返回 1 = 还在打字、2 = 打完待确认、0 = 空闲。剧本用「==1」原地等，
            # 无人值守时按「瞬间打完」处理，直接返回 0。
            return 0 == (1 if arg == 0 else 2)
        if sid == 0x0E:
            # FUN_00435d50：+0x54D；若 +0x54E==1 且 +0x54C==0 再或上 2
            al = self.b54d
            if self.b54e == 1 and self.b54c == 0:
                al |= 2
            return al == arg
        if sid == 0x0F:
            return (g.get(0x801, 0) != 0) == (arg != 0)
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
        if op == 0x30:
            # FUN_00437660：声明位数组尺寸 = 操作数 + 1
            self.bits_n = self.u16() + 1
            return False

        if op == 0x40:
            # FUN_004386a0：声明变量数组尺寸 = 操作数 + 1
            self.vars_n = self.u16() + 1
            return False

        if op == 0xA5:
            # FUN_0043bf90：声明已读文本位图尺寸（存档块 29 的长度）
            n = self.u32raw() + 1
            self.seen_n = n
            return False

        if op == 0xA6:
            # FUN_0043c000：文字窗口参数 <u8> <u16> <u16>
            self.u8(); self.u16(); self.u16()
            return False

        if op == 0x31:
            # FUN_00423ac0：位数组第 bit 位，mode 0 清 / 1 置 / 2 翻转
            bit = self.u16()
            mode = self.u8()
            if bit < len(self.bits):
                if mode == 0:
                    self.bits[bit] = 0
                elif mode == 1:
                    self.bits[bit] = 1
                elif mode == 2:
                    self.bits[bit] ^= 1
            return False
        if op in (0x33, 0x3B):
            # FUN_00437750：if (bit != param) jump；0x33 传 0、0x3B 传 1
            # 即 0x33 = 位为 1 时跳，0x3B = 位为 0 时跳
            bit = self.u16()
            idx = self.u16()
            val = self.bits[bit] if bit < len(self.bits) else 0
            if val == (0 if op == 0x3B else 1):
                self.jump_label(idx)
                return True
            return False
        if op == 0x03:
            # FUN_00436460：模块返回。深度为 0 时就是整个脚本结束（引擎置 error=1）。
            # 非 0 时弹栈、重新载入调用者模块、PC 回到 0x02 之后那条指令。
            if not self.mod_stack:
                raise Halt("END (0x03)")
            name, ret = self.mod_stack.pop()
            self.load_module(name)
            self.code = self.base + ret
            return True
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
            self.reset_timer()
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
            self.events.append(("TEXT", spk, txt, self.voice))
            self.voice = None
            return False
        if op == 0x13:
            # FUN_00436B40：19 个字节
            v = bytes(self.u8() for _ in range(19))
            self.events.append(("RAW13", v.hex()))
            return False
        if op == 0x20:
            # FUN_00436FD0 -> FUN_00414170
            surf = self.u8()
            a, b, c, d = self.val(), self.val(), self.val(), self.val()
            flag = self.u8()
            self.events.append(("SURF_PARAM", surf, a, b, c, d, flag))
            return False
        if op == 0x21:
            # FUN_00437040：<面> <模式> + 最多 4 个名字
            surf = self.u8()
            mode = self.u8()
            names = []
            while self.code < end:
                names.append(self.take_str(end))
            self.events.append(("SURF_IMG", surf, mode, "|".join(names)))
            return False
        if op == 0x23:
            idx = self.u8()
            a, b = self.val(), self.val()
            flag = self.u8()
            self.events.append(("OBJ", idx, a, b, flag))
            return False
        if op == 0x25:
            surf = self.u8()
            flag = self.u8()
            text = self.take_str(end)
            self.code = end
            self.events.append(("SURF_TEXT", surf, flag, text))
            return False
        if op == 0x84:
            # FUN_0043B6F0：<u8 槽> <x> <y> <w> <h> <u8 层> <u16 图> <u8> <u8> <u8>
            slot = self.u8() & 0x3F
            x = self.u32raw()
            y = self.u32raw()
            w = self.u32raw()
            h = self.u32raw()
            layer = self.u8()
            image = self.u16()
            self.u8()
            self.u8()
            self.u8()
            if slot < len(self.slots):
                self.slots[slot] = (x, y, w, h, layer, image)
            return False
        if op == 0x26:
            surf = self.u8()
            on = self.val()
            self.events.append(("SURF_FLAG", surf, on))
            return False
        if op == 0x27:
            surf = self.u8()
            value = self.val()
            self.events.append(("SURF27", surf, value))
            return False
        if op == 0x29:
            self.events.append(("SURF29", self.u8()))
            return False
        if op == 0x2A:
            self.events.append(("SURF2A", self.u8()))
            return False
        if op == 0x73:
            self.events.append(("BGM_STOP",))
            return False
        if op == 0x77:
            self.events.append(("VOICE_PLAY", self.u8()))
            return False
        if op == 0x89:
            self.events.append(("SELECT", self.val() & 0x3F))
            return False
        if op == 0xF7:
            text = self.take_str(end)
            self.code = end
            self.events.append(("TEXT_BUF", text))
            return False
        if op == 0x32:
            frm = self.u16()
            to = self.u16()
            value = self.u8()
            for i in range(frm, min(to + 1, len(self.bits))):
                self.bits[i] = value & 1
            self.events.append(("BITS_FILL", frm, to, value))
            return False
        if op == 0x37:
            kind = self.val() & 0xFF
            arg = self.val() & 0xFF
            self.sys_func(kind, arg)
            self.events.append(("SYSFUNC", kind, arg))
            return False
        if op == 0x45:
            frm = self.u16()
            to = self.u16()
            value = self.val()
            for i in range(frm, min(to + 1, MAX_VARS)):
                self.vars[i] = value
            self.events.append(("VARS_FILL", frm, to, value))
            return False
        if op == 0x48:
            self.code = end
            self.events.append(("SAVE_MENU",))
            return False
        if op == 0x4B:
            arg = self.u16()
            kind = self.u8()
            self.events.append(("SYSCOND", kind, arg))
            return False
        if op == 0x80:
            surf = self.u8()
            name = self.take_str(end)
            self.code = end
            self.events.append(("UI_TEXT", surf, name))
            return False
        if op == 0x86:
            self.events.append(("INPUT_RESET",))
            return False
        if op == 0x87:
            self.events.append(("NOP87",))
            return False
        if op == 0x88:
            idx = self.u8() & 0x3F
            slots = [self.val() & 0x3F for _ in range(8)]
            self.events.append(("COND_GROUP", idx, ",".join(str(v) for v in slots)))
            return False
        if op == 0x8F:
            self.events.append(("COND_CLEAR",))
            return False
        if op == 0xA0:
            self.events.append(("SAVE_MENU_DRAW",))
            return False
        if op == 0xA1:
            v = bytes(self.u8() for _ in range(6))
            self.events.append(("COLOR6", v.hex()))
            return False
        if op == 0xD6:
            surf = self.u8()
            flag = self.u8()
            text = self.take_str(end)
            self.code = end
            self.events.append(("WIN_TEXT", surf, flag, text))
            return False
        if op == 0xE0:
            idx = self.u8()
            text = self.take_str(end)
            self.code = end
            self.events.append(("TEXT_OBJ", idx, text))
            return False
        if op == 0xE1:
            idx = self.u8()
            line = self.u8()
            text = self.take_str(end)
            self.code = end
            self.events.append(("TEXT_OBJ_LINE", idx, line, text))
            return False
        if op in (0xE2, 0xE3):
            idx = self.u8()
            line = self.u8()
            col = self.u8()
            value = self.u16()
            text = self.take_str(end)
            self.code = end
            self.events.append(("TEXT_OBJ_CELL", idx, line, col, value, text))
            return False
        if op == 0xE4:
            v = bytes(self.plain[self.code:end])
            self.code = end
            self.events.append(("TEXT_OBJ_FULL", v.hex()))
            return False
        if op == 0xE5:
            self.events.append(("TEXT_OBJ_FLAG", self.u8()))
            return False
        if op == 0xF1:
            ms = self.val()
            self.deadline = ms
            self.events.append(("DEADLINE", ms))
            return False
        if op == 0xF2:
            self.events.append(("NOP_F2",))
            return False
        if op == 0xF3:
            x, y, w, h = self.val(), self.val(), self.val(), self.val()
            name = self.take_str(end)
            self.code = end
            self.events.append(("MOVIE", x, y, w, h, name))
            return False
        if op == 0xF8:
            surf = self.u8()
            mask = self.u8()
            self.events.append(("COLOR_CH", surf, mask))
            return False
        if op == 0xF9:
            v = bytes(self.u8() for _ in range(5))
            self.events.append(("COLOR5", v.hex()))
            return False
        if op == 0xFA:
            self.events.append(("FUNC_FA", self.val()))
            return False
        if op == 0x7F:
            self.events.append(("BUF_COPY",))
            return False
        if op == 0x60:
            args = [self.val() for _ in range(5)]
            name = self.take_str(end)
            self.code = end
            self.events.append(("EFFECT", ",".join(str(v) for v in args), name))
            return False
        if op == 0x63:
            kind = self.u16()
            a = self.u8()
            b = self.u8()
            dur = self.val()
            self.events.append(("TRANS", kind, a, b, dur))
            return False
        if op == 0x15:
            # FUN_00436D80：选项文本 + 相对窗口基准的矩形
            win = self.u8()
            idx = self.u8()
            x, y, w, h = self.val(), self.val(), self.val(), self.val()
            raw = bytes(self.plain[self.code:end]).split(b"\x00")[0]
            text = raw.decode("cp932", "replace")
            self.code = end
            self.events.append(("CHOICE", win, idx, x, y, w, h, text))
            return False
        if op == 0x16:
            # FUN_00436E60 -> FUN_004182A0：打开选项窗口
            win = self.u8()
            count = self.val()
            flags = self.u8()
            self.events.append(("CHOICE_OPEN", win, count, flags))
            return False
        if op == 0x17:
            # FUN_00436EC0 -> FUN_004183B0：复位窗口
            self.events.append(("CHOICE_CLOSE", self.u8()))
            return False
        if op == 0x18:
            # FUN_00436F00 -> FUN_004186C0：<u8 窗口> <u16 值> <u8 标志>
            win = self.u8()
            value = self.u16()
            flags = self.u8()
            self.events.append(("CHOICE_INPUT", win, value, flags))
            return False
        if op == 0x1A:
            self.events.append(("CHOICE_CLEAR",))
            return False
        if op == 0x1B:
            self.events.append(("CHOICE_SHUFFLE",))
            return False
        if op == 0x74:
            # FUN_0043ABB0：停声道 + 按名字装音效
            nul = self.plain.index(0, self.code, end)
            name = ascii_of(self.plain[self.code:nul])
            self.code = nul + 1
            slot = self.val()
            self.events.append(("SE_LOAD", slot, name))
            return False
        if op == 0x75:
            slot = self.val()
            param = self.val()
            self.events.append(("SE_PLAY", slot, param))
            return False
        if op == 0x76:
            self.events.append(("SE_STOP", self.val()))
            return False
        if op == 0x79:
            self.events.append(("VOICE_PARAM", self.val()))
            return False
        if op == 0x7B:
            slot = self.val()
            value = self.val()
            self.events.append(("SE_REPEAT", slot, value))
            return False
        if op == 0x7E:
            which = self.u8()
            index = self.u16()
            self.events.append(("FLAG_VOICE", which, index))
            return False
        if op == 0x81:
            self.events.append(("UI_MODE", self.u8()))
            return False
        if op == 0x49:
            # FUN_00439000：把全局数组的一段写回存档块 27/28
            start = self.val()
            count = self.val()
            x = self.val()
            which = self.u8()
            self.global_store(start, count, which)
            self.events.append(("GSAVE", which, start, count, x))
            return False
        if op == 0x4A:
            # FUN_00439060：从存档块 27/28 读回全局数组的一段
            start = self.val()
            count = self.val()
            x = self.val()
            which = self.u8()
            self.global_load(start, count, which)
            self.events.append(("GLOAD", which, start, count, x))
            return False
        if op == 0x50:
            # FUN_00439480：<序号> <宽> <高>
            idx = self.val()
            w = self.val()
            h = self.val()
            self.events.append(("SURF", idx, w, h))
            return False
        if op == 0x51:
            # FUN_004394E0：<序号> <x> <y>
            idx = self.val()
            x = self.val()
            y = self.val()
            self.events.append(("SURF_POS", idx, x, y))
            return False
        if op == 0x53:
            # FUN_00439550：<窗口> <r> <g> <b>
            surf = self.val()
            r, g, b = self.u8(), self.u8(), self.u8()
            self.events.append(("WIN_COLOR", surf, r, g, b))
            return False
        if op == 0x55:
            # FUN_00439680：<时长> <r> <g> <b> <u8 标志>
            value = self.val()
            r, g, b, flag = self.u8(), self.u8(), self.u8(), self.u8()
            self.events.append(("WIN_FX", value, r, g, b, flag))
            return False
        if op == 0x57:
            # FUN_004398C0：8 个操作数 + 按 cmd 追加的额外操作数
            cmd = self.u8()
            f = [self.val() for _ in range(8)]
            extra_n = {0x0f: 2, 0x10: 2, 0x11: 2, 0x12: 2, 0x14: 1, 0x15: 1,
                       0x1c: 4, 0x1d: 4}.get(cmd, 0)
            extra = [self.val() for _ in range(extra_n)]
            self.events.append(("DRAW", cmd, f[0], f[1], f[2], f[3], f[4], f[5], f[6], f[7], extra))
            return False
        if op == 0x70:
            # FUN_0043A690：BGM。NUL 结尾的名字 + 模式 u8；引擎把扩展名换成 .OGG
            nul = self.plain.index(0, self.code, end)
            raw = self.plain[self.code:nul].decode("ascii", "replace")
            self.code = nul + 1
            mode = self.u8()
            stem = raw.rsplit(".", 1)[0]
            self.events.append(("BGM", stem + ".OGG", mode))
            return False
        if op == 0x71:
            # FUN_0043A9A0：<u8 循环> <操作数 音量>
            looping = 1 if self.u8() == 0 else 0
            self.events.append(("BGM_VOL", looping, self.val()))
            return False
        if op == 0x72:
            # FUN_0043AAF0：音量操作数，< 1 表示停
            self.events.append(("BGM_LEVEL", self.val()))
            return False
        if op == 0x78:
            self.voice = ascii_of(self.plain[self.code:end])
            self.events.append(("VOICE", self.voice))
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
        if op == 0x56:
            idx = struct.unpack_from("<I", self.plain, self.code)[0]
            name = ascii_of(self.plain[self.code + 4:end])
            self.events.append(("IMAGE", idx, name))
            return False
        if op == 0x01:
            # FUN_004363a0：跳转剧本。指令格式 01 <名字\0>。
            # 先 FUN_00435b00 把「模块深度 / 返回栈 / 模块序号」全清零，再载入新剧本，
            # 所以这是干净的场景切换（不返回）。剧情脚本末尾的 01 "SNR01" 就是下一章。
            s = ascii_of(self.plain[self.code:end])
            self.events.append(("JUMP", s))
            self.ret.clear()
            self.mod_stack.clear()
            if s in self.modules:
                self.load_module(s)
                return True
            return False
        if op == 0x02:
            # FUN_004363e0：调用模块。指令格式 02 <名字\0>，返回地址 = 本指令末尾。
            # 引擎在进入时把调用者名字压到 vm+0x4f+depth*13、返回偏移压到 vm+0x12c，
            # 然后 FUN_00434b50 载入新模块、PC 归零；0x03 时反向恢复。
            s = ascii_of(self.plain[self.code:end])
            self.events.append(("CMDNAME", s))
            if s in self.modules:
                self.mod_stack.append((self.module, end - self.base))
                self.load_module(s)
                return True
            return False
        if op == 0xF0:
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


def load_modules(exe, arc_path):
    """把所有 ISF 成员解密成 {模块名: (plain, head_len, labels)}，供 0x02 调用。"""
    secret = find_secret(open(exe, "rb").read())
    arc = load_archive(arc_path)
    mods = {}
    for name, blob in arc.items():
        if not name.upper().endswith(".ISF"):
            continue
        plain, head_len, _ = decrypt_isf(blob, secret)
        n = (head_len - 8) // 4
        labels = list(struct.unpack_from("<%dI" % n, plain, 8)) if n > 0 else []
        mods[name.rsplit(".", 1)[0].upper()] = (plain, head_len, labels)
    return mods


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
    mods = load_modules(argv[1], argv[2])
    vm = VM(plain, head_len, labels, steps=steps, trace=trace, modules=mods,
            name=key.rsplit(".", 1)[0].upper())
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
        if e[0] == "TEXT":
            print("    (TEXT, %r, %r, %r)" % (e[1], e[2][:40], e[3]))
        else:
            print("   ", e)
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
