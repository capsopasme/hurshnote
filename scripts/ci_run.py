#!/usr/bin/env python3
"""在 CI 中运行命令；失败时把输出尾部作为 GitHub 注解（annotation）上报。

注解可以通过 API 读取（check-runs/{id}/annotations），在无法下载完整日志时用于排查。

用法：python scripts/ci_run.py <标题> -- <命令...>
"""

from __future__ import annotations

import subprocess
import sys

TAIL_LINES = 120
MAX_CHARS = 60000


def escape(s: str) -> str:
    return s.replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")


def main() -> int:
    if "--" not in sys.argv:
        print(__doc__)
        return 2
    sep = sys.argv.index("--")
    title = " ".join(sys.argv[1:sep]) or "command"
    cmd = sys.argv[sep + 1 :]

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
        # 优先报告编译错误所在的行段，否则报告尾部
        err_idx = [i for i, l in enumerate(lines) if l.startswith(("error", "warning: unused")) or "error[" in l or ": error" in l]
        if err_idx:
            start = max(0, err_idx[0] - 5)
            chunk = lines[start : start + TAIL_LINES * 2]
        else:
            chunk = lines[-TAIL_LINES:]
        text = "\n".join(chunk)[-MAX_CHARS:]
        print(f"::error title={escape(title)} 失败（退出码 {code}）::{escape(text)}")
    return code


if __name__ == "__main__":
    sys.exit(main())
