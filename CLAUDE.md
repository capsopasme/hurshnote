# HurshNote 开发约定

原生 Windows 文本编辑器：Rust + Win32（`windows` crate）+ 静态链接的 Scintilla/Lexilla。

开始任何工作前先读：
- `docs/PLAN.md` —— 当前里程碑与任务勾选状态
- `docs/ARCHITECTURE.md` —— crate 划分、线程模型、各子系统设计
- `docs/PITFALLS.md` —— 动手前通读对应模块的小节；发现新坑追加进去

## 硬性规则

- **只有 UI 线程能操作窗口。** 后台线程只能通过 channel + `PostMessage(WM_APP_*)` 回传结果，禁止 `SendMessage`。
- 只有 `hn-sci` 能直接给 Scintilla 发消息。
- 能与平台无关的逻辑放进跨平台 crate（`hn-text`、`hn-search`、`hn-crypto`、`hn-large` 的核心、`hn-md`），保证能在 Linux 上测试。
- 所有 `extern "system"` 回调用 `catch_unwind` 包裹。
- 写用户文件一律走 `hn-core` 的安全保存流程，不要直接 `std::fs::write`。
- 不得复制 Notepad++ 源码（GPL）；Scintilla/Lexilla 源码放在 `third_party/`，不改动上游文件，补丁放 `third_party/patches/`。
- 支持范围：仅 Windows 10 1809+ x64。不要使用 1809 之后才有的 API，必须用时动态加载并提供退路。
- 新增依赖前确认许可证与 MIT 兼容。

## 构建与测试

- Windows：`cargo build --release`；CI 在 `windows-latest` 上构建并上传 exe。
- Linux（云端环境）：`cargo test -p hn-text -p hn-search -p hn-crypto -p hn-large -p hn-md`；Windows 部分用 `cargo xwin check --target x86_64-pc-windows-msvc` 检查。
- 完成一个任务后，在 `docs/PLAN.md` 中勾选；设计有变化时同步更新 `docs/ARCHITECTURE.md`。

## 文档与提交语言

文档、注释、提交信息使用中文；代码标识符使用英文。
