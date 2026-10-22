#!/usr/bin/env python3
"""从 ISF 归档里导出剧本正文（需要配套的 KIZUNAR.EXE 提供密钥表）。

用法：
    python3 isf_text.py <KIZUNAR.EXE> <ISF归档> <输出目录>

输出：每个 .ISF 一个同名 .txt，每行一条文本；行首 [#voice] 表示该句绑定的语音文件。

0x2B 文本指令的 payload 结构（本作实测）：
    00 03           固定头
    ff              叙述（无说话人）
    04 <u8 id>      有说话人，id 为角色槽位
    ff              正文起始标记
    <cp932 文本>     0x00 0x06 = 句中换行；0x00 0x00 = 结束；其余 0x00 为对齐填充
"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from isf_crypto import decrypt_isf, find_secret, load_archive

TEXT_OP = 0x2B
VOICE_OP = 0x78
VOICE_ARCHIVE_OP = 0xF0


def _drop_junk(s):
    """去掉解码后残留的垃圾字符。

    cp932 里 0x80 / 0xA0 / 0xFD..0xFF 这些「未定义单字节」在 Python 里会变成
    私用区 U+F8F0..U+F8F3，在 Rust 的 encoding_rs 里会变成 U+FFFD；两者都当填充丢掉。
    注意必须在**解码之后**按字符过滤：这些字节也可能是合法双字节的续字节（如 あ = 82 A0）。
    """
    return "".join(
        c for c in s
        if not (ord(c) < 0x20 or ord(c) == 0x7F or 0xE000 <= ord(c) <= 0xF8FF or ord(c) == 0xFFFD)
    )


def clean(raw):
    """把 ISF 文本里的控制码翻译成可读字符串（整段按 cp932 解码）。"""
    parts = []
    buf = bytearray()
    i = 0
    while i < len(raw):
        b = raw[i]
        if b == 0x00:
            nxt = raw[i + 1] if i + 1 < len(raw) else 0
            if nxt == 0x06:          # 句中换行
                parts.append(_drop_junk(buf.decode("cp932", "replace")))
                buf.clear()
                parts.append("\n")
                i += 2
                continue
            break                    # 其余 0x00 视为结束/填充
        if b < 0x20:                 # 控制码（0x02 等）直接丢弃；cp932 的续字节不会落在这个区间
            i += 1
            continue
        buf.append(b)
        i += 1
    parts.append(_drop_junk(buf.decode("cp932", "replace")))
    return "".join(parts)


def decode_text(payload):
    """返回 (说话人id 或 None, 文本)。"""
    if len(payload) < 4 or payload[0] != 0 or payload[1] != 3:
        return None, ""
    p = 2
    speaker = None
    if payload[p] == 0xFF:
        p += 1
    elif payload[p] == 0x04 and p + 2 <= len(payload):
        speaker = payload[p + 1]
        p += 2
        if p < len(payload) and payload[p] == 0xFF:
            p += 1
    else:
        p += 1
    return speaker, clean(payload[p:])


def main(argv):
    if len(argv) != 4:
        print(__doc__)
        return 1
    exe_path, arc_path, outdir = argv[1:]
    with open(exe_path, "rb") as f:
        secret = find_secret(f.read())
    archive = load_archive(arc_path)
    os.makedirs(outdir, exist_ok=True)
    total = 0
    for name in sorted(archive):
        if not name.upper().endswith(".ISF"):
            continue
        plain, head_len, ops = decrypt_isf(archive[name], secret)
        if ops is None:
            print("%-14s 解析失败" % name)
            continue
        lines = []
        voice = ""
        for op, payload in ops:
            if op == VOICE_OP:
                voice = payload.split(b"\0")[0].decode("ascii", "replace")
            elif op == TEXT_OP:
                speaker, text = decode_text(payload)
                if not text:
                    continue
                tag = "[#%s] " % voice if voice else ""
                who = "[%02X] " % speaker if speaker is not None else ""
                lines.append(tag + who + text)
        with open(os.path.join(outdir, name + ".txt"), "w", encoding="utf-8") as f:
            f.write("\n".join(lines))
        total += len(lines)
        print("%-14s %5d 条  head_len=%d" % (name, len(lines), head_len))
    print("-- 共 %d 条文本" % total)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
