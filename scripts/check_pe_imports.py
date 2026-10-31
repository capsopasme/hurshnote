#!/usr/bin/env python3
"""列出 PE 文件导入的 DLL，并确认没有依赖 VC++ 运行库（CRT 已静态链接）。

用法：python scripts/check_pe_imports.py target/release/hurshnote.exe
"""

from __future__ import annotations

import re
import struct
import sys

FORBIDDEN = re.compile(r"^(vcruntime|msvcp|ucrtbase|api-ms-win-crt-)", re.I)


def imports(data: bytes) -> list[str]:
    pe = struct.unpack_from("<I", data, 0x3C)[0]
    if data[pe : pe + 4] != b"PE\0\0":
        raise ValueError("不是 PE 文件")
    coff = pe + 4
    nsect, = struct.unpack_from("<H", data, coff + 2)
    opt_size, = struct.unpack_from("<H", data, coff + 16)
    opt = coff + 20
    magic, = struct.unpack_from("<H", data, opt)
    dd = opt + (112 if magic == 0x20B else 96)  # PE32+ / PE32 数据目录偏移
    import_rva, _ = struct.unpack_from("<II", data, dd + 8)
    sections = []
    for i in range(nsect):
        s = opt + opt_size + i * 40
        vsize, vaddr, rsize, raddr = struct.unpack_from("<IIII", data, s + 8)
        sections.append((vaddr, max(vsize, rsize), raddr))

    def off(rva: int) -> int:
        for vaddr, size, raddr in sections:
            if vaddr <= rva < vaddr + size:
                return rva - vaddr + raddr
        raise ValueError(f"RVA {rva:#x} 不在任何节中")

    names = []
    p = off(import_rva)
    while True:
        desc = struct.unpack_from("<IIIII", data, p)
        if not any(desc):
            break
        n = off(desc[3])
        names.append(data[n : data.index(b"\0", n)].decode("ascii"))
        p += 20
    return names


def main() -> int:
    data = open(sys.argv[1], "rb").read()
    dlls = imports(data)
    print("导入的 DLL：" + ", ".join(dlls))
    bad = [d for d in dlls if FORBIDDEN.match(d)]
    if bad:
        print(f"::error title=CRT 未静态链接::依赖 {', '.join(bad)}")
        return 1
    print(f"::notice title=导入的 DLL::{', '.join(dlls)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
