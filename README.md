# HurshNote

一个面向 Windows 的原生文本编辑器，目标是在日常编辑体验上不输 Notepad++，在大文件、数据可靠性和开发者工具上超过它。

> 状态：**早期开发中**。M1（编辑器基础）、M2（可靠性：安全保存、会话快照、文件监控、加密文件）、M3（侧边栏、工作区、分屏、Ctrl+Tab / Ctrl+P）、M4（大文件视图、Hex 视图、Tail 模式、大文件搜索）、M5（底部抽屉、目录搜索与替换、运行 Python、内嵌终端）已完成。
>
> 最新构建：[Nightly 预发布版](https://github.com/capsopasme/hurshnote/releases/tag/nightly)（未签名，首次运行时 SmartScreen 可能拦截，点“更多信息”→“仍要运行”）。

## 主要特性（规划中）

- 原生 Win32 + Scintilla 内核，启动快、体积小、中文输入稳定
- Gruvbox Light 主题，Ctrl+滚轮无级缩放
- 多光标、列选、智能列表续写、行操作、书签批量处理
- 自动编码识别（UTF-8 / UTF-16 / GBK / GB18030 等）与 CRLF/LF 转换
- 无感快照：不保存也能关闭，重启完整恢复
- GB 级大文件秒开（只读视图，5 GB 日志首屏约 40 ms）、Hex 视图、日志 Tail 模式（不影响日志轮转）
- 目录级正则搜索与替换（ripgrep 的遍历组件 + PCRE2，GBK/UTF-16 文件也能搜，替换保留原编码；耗时与 ripgrep 相当）
- 内嵌 PowerShell / pwsh / CMD 终端（ConPTY，支持彩色、补全、中文输入），F5 一键运行 Python，双击 Traceback 跳转
- 原生 Markdown 分栏预览（不依赖浏览器内核）
- 密码加密文件（Argon2id + AES-256-GCM）
- 侧边栏文件列表与工作区目录树，系统原生文件图标

## 文档

- [需求与取舍](docs/REQUIREMENTS.md)
- [架构设计](docs/ARCHITECTURE.md)
- [实施计划](docs/PLAN.md)
- [坑清单](docs/PITFALLS.md)

## 系统要求

Windows 10 1809 及以上，x64。

## 从源码构建

需要 Rust（版本见 `rust-toolchain.toml`）和 Visual Studio Build Tools（“使用 C++ 的桌面开发”）。

```
cargo build --release
```

产物为 `target\release\hurshnote.exe`，单文件，无需安装运行库。

## 许可证

[MIT](LICENSE)
