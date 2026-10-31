#!/usr/bin/env python3
"""下载 Scintilla / Lexilla 官方源码包并放入 third_party/。

用法：
    python scripts/vendor_third_party.py            # 按 third_party/versions.toml
    python scripts/vendor_third_party.py --force    # 即使版本相同也重新下载

versions.toml 中的版本写法："5.5.8" 或 "latest"（从官网下载页解析最新版本）。
解析出的实际版本记录在 third_party/VERSIONS.txt。

只保留编译所需的文件与许可证，其余目录（cocoa/gtk/qt/test/doc 等）删除。
"""

from __future__ import annotations

import argparse
import io
import re
import shutil
import sys
import tomllib
import urllib.request
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
THIRD = ROOT / "third_party"

SOURCES = {
    "scintilla": {
        "page": "https://www.scintilla.org/ScintillaDownload.html",
        "zip": "https://www.scintilla.org/scintilla{ver}.zip",
        "keep": ["License.txt", "README", "version.txt", "include", "src", "win32"],
    },
    "lexilla": {
        "page": "https://www.scintilla.org/LexillaDownload.html",
        "zip": "https://www.scintilla.org/lexilla{ver}.zip",
        "keep": ["License.txt", "README", "version.txt", "include", "lexlib", "lexers", "src"],
    },
}


def fetch(url: str) -> bytes:
    req = urllib.request.Request(url, headers={"User-Agent": "hurshnote-vendor/1.0"})
    with urllib.request.urlopen(req, timeout=120) as resp:
        return resp.read()


def resolve(name: str, wanted: str) -> str:
    """返回紧凑版本号，例如 '558'。"""
    if wanted and wanted != "latest":
        return wanted.replace(".", "")
    html = fetch(SOURCES[name]["page"]).decode("utf-8", "replace")
    found = re.findall(rf"{name}(\d{{3,4}})\.zip", html)
    if not found:
        sys.exit(f"无法从下载页解析 {name} 版本")
    return max(found, key=int)


def dotted(compact: str) -> str:
    return ".".join(compact) if len(compact) == 3 else f"{compact[0]}.{compact[1]}.{compact[2:]}"


def current_version(name: str) -> str | None:
    vf = THIRD / name / "version.txt"
    return vf.read_text().strip() if vf.exists() else None


def vendor(name: str, compact: str) -> None:
    url = SOURCES[name]["zip"].format(ver=compact)
    print(f"下载 {url}")
    data = fetch(url)
    dest = THIRD / name
    if dest.exists():
        shutil.rmtree(dest)
    with zipfile.ZipFile(io.BytesIO(data)) as zf:
        for info in zf.infolist():
            parts = Path(info.filename).parts
            if len(parts) < 2 or parts[0] != name:
                continue
            if parts[1] not in SOURCES[name]["keep"]:
                continue
            if info.is_dir():
                continue
            # 不需要的工程文件
            if Path(info.filename).suffix in {".vcxproj", ".pro", ".ruleset", ".bat"}:
                continue
            target = THIRD.joinpath(*parts)
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(zf.read(info))
    got = current_version(name)
    if got != compact:
        sys.exit(f"{name}: version.txt 为 {got}，期望 {compact}")


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--force", action="store_true")
    args = ap.parse_args()

    cfg = tomllib.loads((THIRD / "versions.toml").read_text(encoding="utf-8"))
    lines = []
    changed = False
    for name in SOURCES:
        compact = resolve(name, str(cfg.get(name, "latest")))
        if args.force or current_version(name) != compact:
            vendor(name, compact)
            changed = True
        else:
            print(f"{name} {dotted(compact)} 已是目标版本")
        lines.append(f"{name} {dotted(compact)}  {SOURCES[name]['zip'].format(ver=compact)}")

    (THIRD / "VERSIONS.txt").write_text("\n".join(lines) + "\n", encoding="utf-8")
    print("changed" if changed else "unchanged")


if __name__ == "__main__":
    main()
