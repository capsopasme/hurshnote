#!/usr/bin/env python3
"""快速生成大日志文件（CI 大文件性能测试用）。

用法：python scripts/gen_big_log.py <输出文件> <大小 MB>

先生成约 1 MB 的一段日志行（含中文），重复写入直到达到指定大小；
每段的行号部分不同，保证文件不是简单的周期重复。
"""

from __future__ import annotations

import sys
import time


def main() -> int:
    if len(sys.argv) != 3:
        print(__doc__)
        return 2
    path = sys.argv[1]
    target = int(sys.argv[2]) * 1024 * 1024
    lines = []
    for i in range(9000):
        lines.append(
            f"2026-11-01 12:{i // 60 % 60:02d}:{i % 60:02d}.{i % 1000:03d} INFO  [worker-{i % 16:02d}] "
            f"请求 id={{:08d}}-{i:05d} 耗时 {i % 997:4d} ms status=200 path=/api/v1/items/{i}\n"
        )
    template = "".join(lines)
    start = time.time()
    written = 0
    block = 0
    with open(path, "wb", buffering=8 * 1024 * 1024) as f:
        while written < target:
            data = template.replace("{:08d}", f"{block:08d}").encode("utf-8")
            if written + len(data) > target:
                data = data[: target - written]
                # 截在行尾
                cut = data.rfind(b"\n")
                if cut < 0:
                    break
                data = data[: cut + 1]
            f.write(data)
            written += len(data)
            block += 1
    secs = time.time() - start
    print(f"写入 {written / 1048576:.0f} MB，用时 {secs:.1f} s")
    return 0


if __name__ == "__main__":
    sys.exit(main())
