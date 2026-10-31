#!/usr/bin/env python3
"""把 main 分支的最新构建发布为 nightly 预发布版（CI 中运行，需要 GH_TOKEN）。

用法：python scripts/publish_nightly.py <exe> <selftest.txt>
"""

from __future__ import annotations

import os
import subprocess
import sys
import zipfile
from pathlib import Path

TAG = "nightly"


def gh(*args: str, check: bool = True) -> subprocess.CompletedProcess:
    r = subprocess.run(["gh", *args], capture_output=True, text=True, encoding="utf-8", errors="replace")
    if check and r.returncode != 0:
        msg = f"gh {' '.join(args[:3])} 失败（{r.returncode}）\n{r.stdout}\n{r.stderr}"
        print(msg)
        print("::error title=发布 nightly::" + msg.replace("%", "%25").replace("\r", "").replace("\n", "%0A"))
        sys.exit(1)
    return r


def main() -> None:
    exe = Path(sys.argv[1])
    selftest = Path(sys.argv[2])
    sha = os.environ.get("GITHUB_SHA", "")
    short = sha[:7]

    zip_path = Path("hurshnote-nightly-x64.zip")
    with zipfile.ZipFile(zip_path, "w", zipfile.ZIP_DEFLATED) as z:
        z.write(exe, "hurshnote.exe")
        z.write("LICENSE", "LICENSE")
        if selftest.exists():
            z.write(selftest, "selftest.txt")

    report = selftest.read_text(encoding="utf-8") if selftest.exists() else "（无）"
    notes = (
        f"main 分支最新构建（{short}）。\n\n"
        "程序未签名，首次运行时 Windows SmartScreen 可能拦截：点“更多信息”→“仍要运行”。\n\n"
        f"自测结果：\n```\n{report}```\n"
    )

    if gh("release", "view", TAG, check=False).returncode == 0:
        gh("release", "delete", TAG, "--yes", "--cleanup-tag")

    gh(
        "release", "create", TAG, str(zip_path), str(exe),
        "--prerelease", "--title", f"Nightly {short}", "--notes", notes, "--target", sha,
    )
    print(f"已发布 nightly（{short}）")


if __name__ == "__main__":
    main()
