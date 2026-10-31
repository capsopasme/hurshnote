#!/usr/bin/env python3
"""生成测试语料。

小样本（入库）：  python scripts/gen_corpus.py
大文件（不入库）：python scripts/gen_corpus.py --large
    生成到 tests/corpus/generated/，包括 100 MB 单行 JSON、1 GB / 5 GB 日志。
"""

from __future__ import annotations

import argparse
import json
import random
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CORPUS = ROOT / "tests" / "corpus"
GENERATED = CORPUS / "generated"

ZH = "汉字编码测试：繁體與简体，标点「」『』……——"
EMOJI = "表情 😀 👨‍👩‍👧 组合字符 é"
GB18030_4BYTE = "\U00020000\U0002A6A5"  # 扩展 B 区，GB18030 中为四字节


def small() -> None:
    CORPUS.mkdir(parents=True, exist_ok=True)
    body = f"第一行 line 1\n{ZH}\n{EMOJI}\n最后一行\n"
    files: dict[str, bytes] = {
        "ascii.txt": b"plain ascii only\nsecond line\n",
        "utf8.txt": body.encode("utf-8"),
        "utf8-bom.txt": b"\xef\xbb\xbf" + body.encode("utf-8"),
        "crlf.txt": body.replace("\n", "\r\n").encode("utf-8"),
        "mixed-eol.txt": "a\r\nb\nc\rd\r\n".encode("utf-8"),
        "gbk.txt": f"第一行\n{ZH}\n".encode("gbk"),
        "gb18030-4byte.txt": f"四字节字符：{GB18030_4BYTE}\n".encode("gb18030"),
        "gbk-short.txt": "中文".encode("gbk"),  # 极短文本，容易误判
        "utf16le-bom.txt": b"\xff\xfe" + body.encode("utf-16-le"),
        "utf16be-bom.txt": b"\xfe\xff" + body.encode("utf-16-be"),
        "utf16le-nobom.txt": body.encode("utf-16-le"),
        "binary.bin": bytes(range(256)) * 4,
        "empty.txt": b"",
        "long-line.txt": ("x" * 200_000 + "\n").encode(),
    }
    for name, data in files.items():
        (CORPUS / name).write_bytes(data)
    print(f"写入 {len(files)} 个小样本到 {CORPUS}")


def large() -> None:
    GENERATED.mkdir(parents=True, exist_ok=True)
    rnd = random.Random(42)

    p = GENERATED / "single-line-100mb.json"
    with p.open("w", encoding="utf-8") as f:
        f.write("[")
        size = 0
        first = True
        while size < 100 * 1024 * 1024:
            item = json.dumps({"id": rnd.randint(0, 10**9), "name": "条目", "v": rnd.random()},
                              ensure_ascii=False, separators=(",", ":"))
            if not first:
                f.write(",")
            f.write(item)
            size += len(item.encode()) + 1
            first = False
        f.write("]")
    print(f"生成 {p}")

    for gb in (1, 5):
        p = GENERATED / f"log-{gb}gb.log"
        target = gb * 1024**3
        levels = ["INFO", "WARN", "ERROR", "DEBUG"]
        with p.open("wb") as f:
            written = 0
            n = 0
            chunk = []
            while written < target:
                line = (f"2026-11-01 12:{n // 60 % 60:02d}:{n % 60:02d}.{n % 1000:03d} "
                        f"[{levels[n % 4]}] worker-{n % 16} 请求处理完成 id={n} "
                        f"耗时={rnd.randint(1, 999)}ms\n").encode()
                chunk.append(line)
                written += len(line)
                n += 1
                if len(chunk) >= 10000:
                    f.write(b"".join(chunk))
                    chunk.clear()
            f.write(b"".join(chunk))
        print(f"生成 {p}")


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--large", action="store_true")
    args = ap.parse_args()
    if args.large:
        large()
    else:
        small()


if __name__ == "__main__":
    main()
