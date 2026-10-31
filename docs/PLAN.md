# HurshNote 实施计划

> 目标：做一个在日常编辑上不输 Notepad++、在大文件、可靠性和开发者工具上全面超过它的 Windows 文本编辑器。
> 需求见 [REQUIREMENTS.md](REQUIREMENTS.md)，设计见 [ARCHITECTURE.md](ARCHITECTURE.md)，坑见 [PITFALLS.md](PITFALLS.md)。

## 已确认的决策

| 项 | 决策 |
|---|---|
| 语言 | Rust（stable，MSVC 工具链） |
| 界面 | 原生 Win32（`windows` crate），不使用 GUI 框架和浏览器内核 |
| 编辑内核 | Scintilla 5 + Lexilla，静态编译进 exe |
| 大文件 | 双层模式；超过 256 MB 进入只读大文件视图（搜索、跳转、Hex、Tail） |
| Markdown | `pulldown-cmark` 解析 + Direct2D/DirectWrite 原生自绘，分栏预览 |
| 加密 | 密码派生：Argon2id + AES-256-GCM |
| 平台 | 仅 Windows 10 1809+ x64 |
| 分发 | 便携 zip + Inno Setup 安装包，不做代码签名 |
| 协议 | MIT；禁止复制 Notepad++（GPL）代码 |

## 开发方式

- 代码在 GitHub 上维护；每次推送由 GitHub Actions（`windows-latest`）编译、测试并上传 exe 产物，在 Windows 电脑上下载试用。
- 云端 Linux 环境可跑跨平台 crate 的测试，并用 `cargo-xwin` 交叉编译检查 Windows 部分能否通过编译；界面行为以 Windows 上实测为准。
- 每个里程碑结束时：跑一遍性能预算（ARCHITECTURE §7），更新本文件的勾选状态。

---

## M0 骨架 ✅

**目标：能编译出一个显示 Scintilla 编辑区的空壳 exe。**

- [x] Cargo workspace、`rust-toolchain.toml`、`.gitignore`、clippy 配置（workspace lints）
- [x] 引入 Scintilla 5.6.8 / Lexilla 5.5.5 源码到 `third_party/`（`vendor` 工作流下载），`hn-sci` 的 `build.rs` 用 `cc` 编译
- [x] `hn-app`：主窗口类、消息循环、manifest（DPI Per-Monitor V2、ComCtl v6、longPathAware）、应用图标、版本信息
- [x] 嵌入一个 Scintilla 视图，DirectWrite 渲染，UTF-8 代码页
- [x] 以 UTF-8 打开/保存单个文件（保持 BOM 与换行符；M2 再换成安全保存）
- [x] Gruvbox Light 基础配色（默认样式、行号栏、当前行、选区、光标）
- [x] 所有窗口过程的 `catch_unwind` 包装，panic 记录到 `%LOCALAPPDATA%\HurshNote\logs\panic.log`
- [x] GitHub Actions：Windows 编译 + clippy + 测试 + `--selftest` 启动自测 + CRT 静态链接检查 + 上传 exe + 发布 nightly；Linux 跑格式与跨平台 crate 测试
- [x] `tests/corpus/` 初始语料与大文件生成脚本（`scripts/gen_corpus.py`）

**验收：** CI 产出单个 exe（静态 CRT，1.5 MB，只依赖系统 DLL）✅；启动到可输入的中位数 60–80 ms（CI 机器，热启动）✅；可输入中文（待在真机上确认）。

> 冷启动（刚下载、首次运行）在 CI 上为 0.5–2 s，主要是磁盘缓存和 Defender 扫描未签名文件，不计入预算。

## M1 编辑器基础

**目标：日常小文件编辑可以完全替代 Notepad++。**

- [ ] `hn-text`：编码嗅探（BOM / UTF-16 启发式 / UTF-8 校验 / chardetng / ANSI）、解码、编码、lossy 检测
- [ ] `hn-text`：EOL 检测（含 Mixed）、转换
- [ ] 后台加载（`SCI_CREATELOADER`），超长行检测与降级
- [ ] 状态栏：行列、选中字符数/行数、编码、EOL、语言、只读、Dirty；点击编码/EOL 弹出切换菜单
- [ ] "以编码重新打开" / "转换为编码"
- [ ] 主题系统：语义样式 + 各 lexer 映射表；首批语言的高亮与折叠
- [ ] 按扩展名/首行（shebang、`<?xml`）识别语言，可手动切换
- [ ] 无级缩放（Ctrl+滚轮、Ctrl+0 复位）
- [ ] 多光标、列选、虚拟空间、多重粘贴配置
- [ ] 行号、折叠栏、缩进线、括号匹配、空白/EOL 显示开关、自动换行开关
- [ ] 列表续写
- [ ] 行操作：排序（升/降/不区分大小写/数字）、去重、Trim、合并、删除空行、移动行
- [ ] 书签：栏点击、Ctrl+F2、F2/Shift+F2 跳转、复制/剪切/删除带书签行
- [ ] 只读切换
- [ ] 查找/替换栏 + 对话框（普通/转义/正则，PCRE2），计数、标记全部、仅选区
- [ ] 菜单与快捷键（尽量沿用 N++ 默认快捷键），快捷键表做成数据
- [ ] 单实例 + 命令行参数；拖放打开；最近文件
- [ ] 多文档基础管理（M3 前先用菜单"窗口"列表切换）

**验收：** 用语料逐一打开、编辑、保存，字节级与预期一致；常用操作与 N++ 对照无缺失。

## M2 可靠性

**目标：任何时候强杀进程都不丢数据，任何保存都不损坏文件。**

- [ ] 安全保存流程（临时文件 + `ReplaceFileW`，硬链接/符号链接/失败退化）
- [ ] 只读属性、受保护路径提权保存（`--elevated-save`）
- [ ] 快照：防抖写入、原子替换、会话清单
- [ ] 会话恢复：懒加载、光标/滚动/折叠/书签恢复、磁盘变化冲突提示
- [ ] 关闭时不询问保存（可配置）
- [ ] 文件监控：父目录监听、去抖、自身写入过滤、溢出全量比对、网络盘轮询、删除检测
- [ ] 崩溃处理：未处理异常时写 `MiniDumpNormal`，下次启动提示并从快照恢复
- [ ] `hn-crypto`：加密格式 v1 读写 + 单元测试 + 模糊测试（界面在 M6，格式和快照加密在此定稿）
- [ ] 加密文档快照
- [ ] 强杀测试脚本接入 CI

**验收：** 强杀测试连续 500 轮零丢失；保存过程中断电模拟（在各步骤间注入失败）后原文件完好。

## M3 外壳

**目标：多文件工作流比标签页更顺手。**

- [ ] 侧边栏框架（可拖动宽度、可隐藏）
- [ ] "打开的文件"列表：系统图标、Dirty/只读/加密标记、拖动排序、右键菜单（关闭、关闭其他、在资源管理器中显示、复制路径）
- [ ] 工作区树：添加文件夹、懒加载、`.gitignore` 过滤、文件监控刷新、新建/重命名/删除（回收站）
- [ ] 图标提取后台线程与缓存
- [ ] Ctrl+Tab MRU 切换器
- [ ] Ctrl+P 快速打开（模糊匹配）
- [ ] 分屏：左右/上下，Clone View（同一文档）与两个不同文档并排
- [ ] 布局持久化到会话
- [ ] 设置文件与便携模式

**验收：** 打开 50 个文件切换流畅；工作区 10 万文件的目录展开不卡。

## M4 大文件

**目标：GB 级文件秒开。**

- [ ] `hn-large`：分块读取 / mmap + SEH 包裹、稀疏行索引后台构建
- [ ] `hn-render` 共享渲染设施
- [ ] 大文件视图：虚拟滚动、跳转到行、选中复制、编码选择
- [ ] 大文件流式搜索，结果列表跳转
- [ ] Hex 视图（`ByteSource` 两种实现），与文本视图切换并保持位置
- [ ] Tail 模式（`ReadFile` 增量读、轮转检测、自动滚到底、暂停）
- [ ] 二进制文件检测与提示

**验收：** 5 GB 日志首屏 < 300 ms、滚动不掉帧；对正在写入的日志开 Tail 时，写日志程序的轮转不受影响。

## M5 开发者工具

**目标：常见开发辅助不离开编辑器。**

- [ ] 底部抽屉：输出 / 终端 / 搜索结果三个页签，可调高度，快捷键呼出
- [ ] 目录搜索：对话框、过滤器（扩展名、排除目录）、结果树（虚拟列表）、点击跳转、取消
- [ ] 目录替换（预览 + 安全写入）
- [ ] Python 运行：解释器探测、Job Object、实时输出、Traceback 跳转、停止、在终端中运行
- [ ] 终端：ConPTY 封装、VT 状态机接入、D2D 网格渲染、IME、复制粘贴、resize、多个终端实例、shell 选择
- [ ] 评估并接入随附版 ConPTY

**验收：** 目录搜索耗时与 ripgrep 差距 < 30%；终端中 vim、PSReadLine 补全、彩色输出正常；中文输入正常。

## M6 内容

- [ ] Markdown 预览：块树、DirectWrite 排版、表格、图片、链接、滚动同步、基础复制
- [ ] 加密界面：另存为加密、设置/修改密码、打开时输入密码、错误提示、首次使用警告
- [ ] 宏录制与回放
- [ ] 打印（基于 Scintilla `SCI_FORMATRANGE`）

**验收：** 10 万字 Markdown 编辑时预览更新不卡顿；加密文件跨机器可打开。

## M7 发布 1.0

- [ ] 性能预算全面回归，必要时优化
- [ ] 模糊测试长时间运行（编码、MD、加密格式、行索引）
- [ ] 便携 zip、Inno Setup 安装包（HKCU 文件关联与经典右键菜单、开始菜单、卸载）
- [ ] `THIRD_PARTY_NOTICES.txt`
- [ ] README（含 SmartScreen 说明）、用户手册、更新日志
- [ ] GitHub Release 自动化（打 tag 即发布）

---

## v1 之后（候选）

- 大文件可编辑（分块 piece table 引擎）
- Hex 覆盖式编辑
- 撤销历史持久化
- 代码块语法高亮（预览中）
- 文本对比（diff 视图）
- 自定义快捷键界面、更多主题（Gruvbox Dark 等）
- 插件机制（Lua 或 WASM）
