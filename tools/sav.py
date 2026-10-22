"""KIZUNAR.SAV 容器解析 / 构造。

布局见 docs/06-save.md，全部来自 KIZUNAR.EXE 的存档代码：
  0x00  u32 offset[30]  块偏移表；块 i = [offset[i], offset[i+1])，末块到文件尾
        块 0..26 = 27 个存档格，块 27/28 = 引擎保留块，块 29 = 已读文本位图

存档格：
  +0x0000  SYSTEMTIME 16B
  +0x0010  缩略图 0xE128B（40B BITMAPINFOHEADER + 160x120x24bpp）
  +0xE138  标题 0xA0B（cp932）
  +0xE1D8  5 x (u16,u16) 扩展描述（恒 0）
  +0xE1EC  位数组 129B（1025 位）
  +0xE26D  变量数组 542 x u32
  +0xEAE5  尾部 0x30B 0
"""

import struct
import sys

TABLE_N = 30
SLOT_N = 27
TABLE_BYTES = TABLE_N * 4

THUMB_W = 160
THUMB_H = 120
THUMB_HDR = 40
THUMB_SIZE = THUMB_HDR + THUMB_W * THUMB_H * 3      # 0xE128

TITLE_SIZE = 0xA0
EXTRA_SIZE = 20
BITS_N = 1025                                        # 0x30 0x0400 -> n+1
BITS_BYTES = (BITS_N + 7) // 8                       # 129
VARS_N = 542                                         # 0x40 0x021D -> n+1
TAIL_SIZE = 0x30

SLOT_SIZE = 0x10 + THUMB_SIZE + TITLE_SIZE + EXTRA_SIZE + BITS_BYTES + VARS_N * 4 + TAIL_SIZE
SYSTEM_BLOCKS = (27, 28)
GLOBAL_BLOCK = 29
GLOBAL_BITS_N = 30001                                # 0xA5 0x7530 -> n+1


class Slot:
    def __init__(self):
        self.time = (0,) * 8
        self.thumb = bytes(THUMB_SIZE)
        self.title = ""
        self.extra = bytes(EXTRA_SIZE)
        self.bits = bytes(BITS_BYTES)
        self.vars = [0] * VARS_N

    @property
    def used(self):
        return any(self.time)

    def sort_key(self):
        y, mo, _dow, d, h, mi, s, _ms = self.time
        return (y * 0x10000 + mo * 0x100 + d, h * 0x10000 + mi * 0x100 + s)

    def set_time(self, t):
        """t = time.struct_time 或 8 元组。"""
        if hasattr(t, "tm_year"):
            t = (t.tm_year, t.tm_mon, (t.tm_wday + 1) % 7, t.tm_mday,
                 t.tm_hour, t.tm_min, t.tm_sec, 0)
        self.time = tuple(int(x) & 0xFFFF for x in t)

    def __repr__(self):
        y, mo, _dow, d, h, mi, s, _ms = self.time
        return "<Slot %04d-%02d-%02d %02d:%02d:%02d %r>" % (y, mo, d, h, mi, s, self.title)


class SaveFile:
    def __init__(self):
        self.slots = [Slot() for _ in range(SLOT_N)]
        self.system = {}          # 27/28 -> bytes
        self.global_bits = b""    # 块 29 的 RLE 负载（不含长度前缀）

    def newest(self):
        best = None
        for i, s in enumerate(self.slots):
            if not s.used:
                continue
            if best is None or s.sort_key() > self.slots[best].sort_key():
                best = i
        return best


def parse(data):
    if len(data) < TABLE_BYTES:
        raise ValueError("SAV 太短：%d 字节" % len(data))
    off = list(struct.unpack_from("<%dI" % TABLE_N, data, 0))
    sf = SaveFile()
    # 空块的偏移是 0，所以块的结束位置要看「下一个非零偏移」
    ends = [len(data)] * TABLE_N
    nxt = len(data)
    for i in range(TABLE_N - 1, -1, -1):
        ends[i] = nxt
        if off[i]:
            nxt = off[i]
    for i in range(TABLE_N):
        start, end = off[i], ends[i]
        if start == 0 or start >= end or end > len(data):
            continue
        blob = data[start:end]
        if i < SLOT_N:
            sf.slots[i] = _parse_slot(blob)
        elif i in SYSTEM_BLOCKS:
            sf.system[i] = blob
        elif i == GLOBAL_BLOCK:
            n = struct.unpack_from("<I", blob, 0)[0]
            sf.global_bits = blob[4:4 + n]
    return sf


def _parse_slot(blob):
    if len(blob) < SLOT_SIZE:
        raise ValueError("存档格太短：%d（应为 %d）" % (len(blob), SLOT_SIZE))
    s = Slot()
    s.time = struct.unpack_from("<8H", blob, 0)
    s.thumb = blob[0x10:0x10 + THUMB_SIZE]
    raw = blob[0xE138:0xE138 + TITLE_SIZE]
    s.title = raw.split(b"\0", 1)[0].decode("cp932", "replace")
    p = 0xE138 + TITLE_SIZE
    s.extra = blob[p:p + EXTRA_SIZE]
    p += EXTRA_SIZE
    s.bits = blob[p:p + BITS_BYTES]
    p += BITS_BYTES
    s.vars = list(struct.unpack_from("<%di" % VARS_N, blob, p))
    return s


def _build_slot(s):
    return (struct.pack("<8H", *s.time)
            + s.thumb[:THUMB_SIZE].ljust(THUMB_SIZE, b"\0")
            + s.title.encode("cp932")[:TITLE_SIZE].ljust(TITLE_SIZE, b"\0")
            + s.extra[:EXTRA_SIZE].ljust(EXTRA_SIZE, b"\0")
            + s.bits[:BITS_BYTES].ljust(BITS_BYTES, b"\0")
            + struct.pack("<%di" % VARS_N, *s.vars[:VARS_N])
            + bytes(TAIL_SIZE))


def build(sf):
    blocks = [(_build_slot(s) if s.used else b"") for s in sf.slots]
    blocks.append(sf.system.get(27, b""))
    blocks.append(sf.system.get(28, b""))
    g = sf.global_bits
    blocks.append((struct.pack("<I", len(g)) + g) if g else b"")
    return pack_blocks(blocks)


def pack_blocks(blocks):
    off = []
    pos = TABLE_BYTES
    body = bytearray()
    for b in blocks:
        if b:
            off.append(pos)
            body += b
            pos += len(b)
        else:
            off.append(0)
    off += [0] * (TABLE_N - len(off))
    return struct.pack("<%dI" % TABLE_N, *off) + bytes(body)


def thumb_from_rgb(rgb, w, h):
    """把 w*h*3 的 RGB 缩到 160x120 并包成 BITMAPINFOHEADER + BGR 顶朝下。"""
    out = bytearray(THUMB_HDR + THUMB_W * THUMB_H * 3)
    struct.pack_into("<IiiHHIIiiII", out, 0, THUMB_HDR, THUMB_W, -THUMB_H, 1, 24, 0, 0, 0, 0, 0, 0)
    for y in range(THUMB_H):
        sy = min(h - 1, y * h // THUMB_H)
        for x in range(THUMB_W):
            sx = min(w - 1, x * w // THUMB_W)
            s = (sy * w + sx) * 3
            d = THUMB_HDR + (y * THUMB_W + x) * 3
            out[d] = rgb[s + 2]
            out[d + 1] = rgb[s + 1]
            out[d + 2] = rgb[s]
    return bytes(out)


def _bit_total(nbits):
    """引擎按字节覆盖：实际编码/解码的位数要向上取整到 8 的倍数。"""
    return ((nbits + 7) // 8) * 8


def rle_decode(data, nbits):
    nbits = _bit_total(nbits)
    out = bytearray((nbits + 7) // 8)
    pos = 0
    bit = 0
    i = 0
    while i < len(data) and bit < nbits:
        b0 = data[i]
        cls = (b0 & 0x60) >> 5
        n = b0 & 0x1F
        i += 1
        for _ in range(cls):
            n = (n << 8) | data[i]
            i += 1
        val = (b0 & 0x80) != 0
        for _ in range(n):
            if bit >= nbits:
                break
            if val:
                out[bit >> 3] |= 1 << (bit & 7)
            bit += 1
    return bytes(out)


def rle_encode(bits, nbits):
    nbits = _bit_total(nbits)
    out = bytearray()
    i = 0
    while i < nbits:
        v = (bits[i >> 3] >> (i & 7)) & 1
        j = i
        while j < nbits and ((bits[j >> 3] >> (j & 7)) & 1) == v:
            j += 1
        n = j - i
        while n:
            if n < 0x20:
                out.append((v << 7) | n)
                n = 0
            elif n < 0x2000:
                out += bytes([(v << 7) | 0x20 | (n >> 8), n & 0xFF])
                n = 0
            elif n < 0x200000:
                out += bytes([(v << 7) | 0x40 | (n >> 16), (n >> 8) & 0xFF, n & 0xFF])
                n = 0
            else:
                chunk = min(n, 0x1FFFFFFF)
                out += bytes([(v << 7) | 0x60 | (chunk >> 24), (chunk >> 16) & 0xFF,
                              (chunk >> 8) & 0xFF, chunk & 0xFF])
                n -= chunk
        i = j
    return bytes(out)


def dump(path):
    sf = parse(open(path, "rb").read())
    for i, s in enumerate(sf.slots):
        if s.used:
            print("%2d %r" % (i, s))
    print("最新存档：", sf.newest())
    print("保留块：", {k: len(v) for k, v in sf.system.items()})
    print("已读位图：%d 字节 RLE" % len(sf.global_bits))
    return sf


def selftest(path):
    d = open(path, "rb").read()
    sf = parse(d)
    again = build(sf)
    assert again == d, "回环失败：%d != %d 字节" % (len(again), len(d))
    bits = rle_decode(sf.global_bits, GLOBAL_BITS_N)
    assert rle_encode(bits, GLOBAL_BITS_N) == sf.global_bits, "RLE 回环失败"
    print("OK  %s  %d 字节  块27=%d 块28=%d  已读位图=%d 位"
          % (path, len(d), len(sf.system.get(27, b"")), len(sf.system.get(28, b"")), GLOBAL_BITS_N))


if __name__ == "__main__":
    if len(sys.argv) >= 3 and sys.argv[1] == "--selftest":
        selftest(sys.argv[2])
    elif len(sys.argv) >= 2:
        dump(sys.argv[1])
    else:
        print(__doc__)