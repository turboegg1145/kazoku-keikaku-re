#!/usr/bin/env python3
"""ISF 脚本解密 + 字节码切分（GDT / Ikura GDL）。

分两层，必须按顺序解：

1) 外层 "SECRETFILTER100a" 异或壳
   - 文件末尾 16 字节是校验/长度尾巴，先丢掉。
   - 密钥不是常量，而是从 EXE 里 2048 字节的 "UOB0..." 表实时算出来的：
       密钥长度 = 由 secret[0x500..0x501] 与 secret[0x100..0x101] 的差推出
       密钥内容 = secret[0x510+i] - secret[0x110+i]（经 HEX_TABLE/HEX_ENCODE_MAP 编码）
       每 0x1000 字节（klen 的整数倍）用 secret[(index & 0x3F) * 0x10 + i] 轮换密钥
   - 本作 KIZUNAR.EXE 中该表位于文件偏移 0x74218。

2) 内层 ikuar_decrypt
   - 按 data[4:6] 的版本魔数变换 offset 8 之后的全部字节：
       0x9795 -> 每字节循环右移 2 位
       0xD197 -> 每字节按位取反
       0xCE89 -> 与 data[6] 异或

解密后：
    u32 head_len
    u32 version (0x9795)
    (head_len-8)/4 个 u32 偏移表
    从 head_len 开始是字节码流：opcode(u8) + len(u8) [+ ext]
        len < 0x80 -> 头部 2 字节
        len == 0x80 -> 第 3 字节是长度
        len == 0x81 -> 第 3 字节 + 0x100
    len 含头部自身，payload = len - 头部字节数。
"""
import struct

HEX_ENCODE_MAP = b"G5FXIL094MPRKWCJ3OEBVA7HQ2SU8Y6TZ1ND"
HEX_TABLE = [
    0x06, 0x21, 0x19, 0x10, 0x08, 0x01, 0x1E, 0x16, 0x1C, 0x07, 0x15, 0x13,
    0x0E, 0x23, 0x12, 0x02, 0x00, 0x17, 0x04, 0x0F, 0x0C, 0x05, 0x09, 0x22,
    0x11, 0x0A, 0x18, 0x0B, 0x1A, 0x1F, 0x1B, 0x14, 0x0D, 0x03, 0x1D, 0x20,
]


def chr2hex(c):
    if 48 <= c <= 57:
        return c - 48
    if 97 <= c <= 122:
        return c - 97 + 10
    if 65 <= c <= 90:
        return c - 65 + 10
    return 0


def chr2hexcode(c):
    return HEX_TABLE[chr2hex(c)]


def encode_hex(sym):
    if sym > 127:
        sym -= 256
    return HEX_ENCODE_MAP[sym % 36]


def str2hex(s):
    v = 0
    for i in range(len(s)):
        v |= chr2hex(s[i]) << ((len(s) - i - 1) << 2)
    return v


def create_key(secret):
    length = bytearray(2)
    for i in range(2):
        length[i] = encode_hex((chr2hexcode(secret[0x500 + i]) - chr2hexcode(secret[0x100 + i])) & 0xFF)
    klen = str2hex(length)
    key = bytearray(klen)
    for i in range(klen):
        key[i] = encode_hex((chr2hexcode(secret[0x510 + i]) - chr2hexcode(secret[0x110 + i])) & 0xFF)
    return key


def update_key(secret, key, index):
    p = (index & 0x3F) * 0x10
    for i in range(len(key)):
        key[i] = encode_hex((chr2hexcode(key[i]) + chr2hexcode(secret[p + i])) & 0xFF)


def handle_isf_xor(data, secret):
    key = bytearray(create_key(secret))
    klen = len(key)
    out = bytearray(data)
    for i in range(len(out)):
        if i % klen == 0:
            update_key(secret, key, i // klen)
        out[i] ^= key[i % klen]
    return bytes(out)


def ikuar_decrypt(data):
    out = bytearray(data)
    ver = int.from_bytes(out[4:6], "little")
    if ver == 0x9795:
        for i in range(8, len(out)):
            b = out[i]
            out[i] = (b >> 2) | ((b << 6) & 0xFF)
    elif ver == 0xD197:
        for i in range(8, len(out)):
            out[i] = (~out[i]) & 0xFF
    elif ver == 0xCE89:
        k = out[6]
        for i in range(8, len(out)):
            out[i] ^= k
    return bytes(out)


def find_secret(exe_bytes):
    """从 EXE 中取出 2048 字节的 UOB0 密钥表。"""
    pos = exe_bytes.find(b"UOB0")
    if pos < 0:
        raise ValueError("EXE 中找不到 UOB0 密钥表")
    return exe_bytes[pos:pos + 2048]


def parse_ops(body):
    """把字节码流切成 [(opcode, payload), ...]；失败返回 None。"""
    p = 0
    ops = []
    n = len(body)
    while p < n:
        if p + 2 > n:
            return None
        op = body[p]
        ln = body[p + 1]
        head = 2
        if ln >= 0x82:
            return None
        if ln == 0x80:
            if p + 3 > n:
                return None
            ln = body[p + 2]
            head = 3
        elif ln == 0x81:
            if p + 3 > n:
                return None
            ln = body[p + 2] + 0x100
            head = 3
        if ln < head or p + ln > n:
            return None
        ops.append((op, body[p + head:p + ln]))
        p += ln
    return ops


def decrypt_isf(raw, secret):
    """raw 为归档内的单文件字节；返回 (完整明文, head_len, ops)。"""
    plain = ikuar_decrypt(handle_isf_xor(raw[:-16], secret))
    head_len = struct.unpack_from("<I", plain, 0)[0]
    return plain, head_len, parse_ops(plain[head_len:])


def load_archive(path):
    """读 SM2MPX10 归档，返回 {名字: 字节}。"""
    with open(path, "rb") as f:
        d = f.read()
    n = struct.unpack_from("<I", d, 8)[0]
    out = {}
    for i in range(n):
        o = 0x20 + i * 20
        nm = d[o:o + 12].split(b"\0")[0].decode("cp932", "replace")
        off, sz = struct.unpack_from("<II", d, o + 12)
        out[nm] = d[off:off + sz]
    return out
