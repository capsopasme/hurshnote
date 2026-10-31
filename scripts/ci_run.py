#!/usr/bin/env python3
"""在 CI 中运行命令；失败时把编译错误逐条作为 GitHub 注解（annotation）上报。

注解可以通过 API 读取（check-runs/{id}/annotations），在无法下载完整日志时用于排查。
每条注解消息长度有限（约 4 KB），每个步骤最多上报 MAX_BLOCKS 条。

用法：python scripts/ci_run.py <标题> -- <命令...>
"""

from __future__ import annotations

import re
import subprocess
import sys

MAX_BLOCKS = 9
MAX_CHARS = 3800
TAIL_LINES = 60

START = re.compile(r"^(error|warning)(\[\w+\])?:|^\S.*: (error|fatal error) [A-Z]*\d+|^---- .* stdout ----")


def escape(s: str) -> str:
    return s.replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")


def blocks(lines: list[str]) -> list[str]:
    """按 rustc/cl 输出格式切出每条诊断（从 error/warning 行到空行）。"""
    out: list[str] = []
    cur: list[str] | None = None
    for l in lines:
        if START.search(l):
            if cur:
                out.append("\n".join(cur))
            cur = [l]
        elif cur is not None:
            if l.strip() == "":
                out.append("\n".join(cur))
                cur = None
            else:
                cur.append(l)
    if cur:
        out.append("\n".join(cur))
    # 过滤掉汇总行（"error: could not compile"、"warning: build failed" 等）
    skip = ("could not compile", "build failed", "test failed, to rerun", "warning: unused manifest key")
    return [b for b in out if not any(k in b.splitlines()[0] for k in skip)]


def main() -> int:
    if "--" not in sys.argv:
        print(__doc__)
        return 2
    sep = sys.argv.index("--")
    title = " ".join(sys.argv[1:sep]) or "command"
    cmd = sys.argv[sep + 1:]

    proc = subprocess.Popen(
        cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
        encoding="utf-8", errors="replace",
    )
    lines: list[str] = []
    assert proc.stdout is not None
    for line in proc.stdout:
        sys.stdout.write(line)
        lines.append(line.rstrip("\n"))
    code = proc.wait()
    sys.stdout.flush()

    if code != 0:
        bs = blocks(lines)
        # 错误优先，其次警告；去重
        seen = set()
        first = [b for b in bs if b.startswith(("error", "----"))]
        ordered = first + [b for b in bs if b not in first]
        uniq = []
        for b in ordered:
            if b not in seen:
                seen.add(b)
                uniq.append(b)
        if not uniq:
            uniq = ["\n".join(lines[-TAIL_LINES:])]
        print(f"共 {len(uniq)} 条诊断")
        for i, b in enumerate(uniq[:MAX_BLOCKS]):
            print(f"::error title={escape(title)} {i + 1}/{len(uniq)}::{escape(b[:MAX_CHARS])}")
    return code


if __name__ == "__main__":
    sys.exit(main())
