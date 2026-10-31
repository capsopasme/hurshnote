# 坑清单

每个模块开发前通读对应小节；代码评审时逐条核对。发现新坑就追加到这里。

## 通用 / Win32

- [ ] 后台线程向窗口 `SendMessage` 会死锁或产生竞态 → 只用 channel + `PostMessage`（见 ARCHITECTURE §4）。
- [ ] 后台结果逐条 `PostMessage` 会淹没消息队列 → 合批，16–50 ms 一次。
- [ ] manifest 必须声明：Per-Monitor V2 DPI、Common Controls v6、`longPathAware`、`supportedOS`（Win10）。**不要**声明 `activeCodePage = UTF-8`，否则 `GetACP()` 返回 65001，无法得到用户真实的 ANSI 代码页（GBK 文件的兜底判断依赖它）。
- [ ] `WM_DPICHANGED` 要按建议矩形重设窗口并重建字体；自绘部分一律用 DIP 计算。
- [ ] 所有 Win32 字符串用 UTF-16（`W` 版本 API），不要用 `A` 版本。
- [ ] 路径超过 260 字符：manifest 的 `longPathAware` 需要系统注册表开启才生效，因此内部对长路径统一加 `\\?\` 前缀；`\\?\` 路径不做 `.`/`..` 规范化，传入前先 `GetFullPathNameW`。
- [ ] 不签名的 exe 首次运行会被 SmartScreen 拦截，Rust 程序偶尔被 Defender 误报 → README 写明；必要时提交微软误报申诉。
- [ ] Rust 中不能直接用 SEH → 需要捕获结构化异常的地方（mmap 页访问）写成小的 C 函数，通过 `cc` 编译。
- [ ] `panic` 跨越 FFI 回调边界（窗口过程、Scintilla 通知）是未定义行为 → 所有 `extern "system"` 回调包 `catch_unwind`。

## Scintilla

- [ ] 静态链接要调用 `Scintilla_RegisterClasses(hInstance)`；Lexilla 静态链接用 `CreateLexer`，不用 `LoadLexerLibrary`。
- [ ] `SCI_SETZOOM` 是整数步进，做无级缩放要用 `SCI_STYLESETSIZEFRACTIONAL`，并且每次换 lexer/主题后重新应用。
- [ ] 换 lexer 后样式会被重置 → 顺序：`SCI_SETILEXER` → `SCI_STYLERESETDEFAULT` → 设默认样式 → `SCI_STYLECLEARALL` → 设各语义样式 → 设属性（`fold` 等）→ `SCI_COLOURISE(0, -1)`。
- [ ] 超长单行（压缩 JSON）让排版和 lexer 极慢 → 打开时测最长行，超过阈值关闭折行、lexer、缩进线；`SCI_SETLAYOUTCACHE(SC_CACHE_PAGE)`。
- [ ] `SCI_GETCHARACTERPOINTER` 会移动 gap，返回的指针在下一次修改后失效 → 立即使用或拷贝，不能跨消息持有。
- [ ] 默认 `SCI_SETCODEPAGE` 不是 UTF-8 → 每个新文档都设 `SC_CP_UTF8`。
- [ ] DirectWrite 模式下某些字体/渲染选项与 GDI 模式表现不同（如粗体宽度）→ 统一只用 DirectWrite 测试。
- [ ] 多视图共享文档时，每个视图的折叠、选区、滚动、书签显示是独立的，但 marker 存在文档上 → 书签在两个视图同时可见（这是期望行为），折叠状态需要各视图分别保存。
- [ ] 撤销分组：自定义命令（行操作、列表续写、全部替换）必须包在 `SCI_BEGINUNDOACTION` / `SCI_ENDUNDOACTION` 中。
- [ ] Ctrl+滚轮默认会触发 Scintilla 自带缩放 → 拦截后走我们的小数缩放逻辑，并同步到所有视图。
- [ ] Ctrl+点击只有在 `SCI_SETMULTIPLESELECTION(true)` 时才添加光标；Ctrl+双击、Ctrl+拖拽与 Alt+拖拽的组合行为要逐一实测，并与"Ctrl+点击打开链接"之类的功能避免冲突。
- [ ] 中文 IME：Scintilla 已处理内联输入；不要在 `WM_IME_*` 上拦截 Scintilla 窗口的消息。
- [ ] lexer、关键字、属性、EOL 模式、只读、代码页属于**文档**；样式、边栏、标记定义、换行模式属于**视图**。切换文档（`SCI_SETDOCPOINTER`）后必须重新应用样式，并对新文档设一次 `SC_CP_UTF8`。
- [ ] `SCI_GETMODIFY` 只反映当前文档；非当前文档的修改状态要自己缓存（保存点通知 + 切换时记录）。
- [ ] Scintilla 的修改历史标记占用 21–24 号、折叠占用 25–31 号，书签用 18 号。
- [ ] 边栏宽度、光标宽度是像素值，不随 DPI 缩放，要自己乘 DPI/96。
- [ ] Scintilla 在 `WM_CREATE` 里 `RegisterDragDrop`，需要已 `OleInitialize`；它的拖放目标不接受 CF_HDROP，初始化 OLE 后文件拖到编辑区会失效。
- [ ] `SCN_CHARADDED` 在 CRLF 模式下按 `\r`、`\n` 各发一次，换行后的处理只在 `\n` 时做（CR 模式在 `\r`）。

## 编码

- [ ] 二进制文件按 Windows-1252 解码：encoding_rs 中它对 256 个字节值一一映射（0x80–0x9F 未定义字节映射到 C1 控制字符），往返无损。
- [ ] encoding_rs 不编码 UTF-16（WHATWG 规定输出编码为 UTF-8），UTF-16 编码要自己写。
- [ ] encoding_rs 默认的 `encode()` 会把不可映射字符写成 `&#NNNN;`，必须用 `encode_from_utf8_without_replacement` 自己处理并计数。

- [ ] 短文本 GBK 可能恰好是合法 UTF-8 → 严格 UTF-8 校验 + 允许用户手动指定，并按路径记住。
- [ ] 解码时出现替换字符 → 标记 lossy，保存前警告，否则会静默毁坏原文件。
- [ ] "以编码重新打开"与"转换为编码"语义不同，命令必须分开。
- [ ] UTF-8 BOM 保存时要保持原样（有则有，无则无），除非用户改。
- [ ] GB18030 四字节字符、UTF-16 代理对、组合字符（emoji ZWJ 序列）都要在语料中覆盖。
- [ ] 编码嗅探只读前 64 KB；对于前面纯 ASCII、后面才出现中文的大文件，需要对 ASCII 前缀之后的部分再采样。

## 保存 / 文件 IO

- [ ] 直接覆盖写入时崩溃会留下半截文件 → 临时文件 + `FlushFileBuffers` + `ReplaceFileW`。
- [ ] `ReplaceFileW` 会断开硬链接、替换符号链接本身 → 先检测，特殊情况走"备份 + 原地写"。
- [ ] `ReplaceFileW` 在跨卷、部分网络盘、目标被别人无 `FILE_SHARE_DELETE` 打开时失败 → 有退化路径。
- [ ] 临时文件必须与目标同目录（同卷），名字不能被 `.gitignore` 之外的工具误认（如 `~hn.<随机>.tmp`）。
- [ ] 保存到受保护路径：检测 `ERROR_ACCESS_DENIED` 后才提权，不要预先要求管理员权限运行整个程序。
- [ ] 只读属性文件：提示"文件为只读，是否去掉只读属性后保存"。
- [ ] 保存前文件已被外部修改 → 提示覆盖冲突，不要静默覆盖。
- [ ] 打开文件时共享模式放宽（`FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE`），读完立即关闭句柄，不要长期占用。

## 快照 / 会话

- [ ] 快照写入期间崩溃 → 先写 `.tmp` 再原子替换；清单最后写。
- [ ] 快照恢复后磁盘文件已变化 → 比对 `disk_stamp` 提示，不要默默用旧快照覆盖。
- [ ] 加密文档的快照必须加密。
- [ ] 未命名文档也要恢复（含其临时标题"新建 3"）。
- [ ] 快照目录要有清理：没有被清单引用的 `.snap` 在启动时删除（延后执行，先确保恢复成功）。
- [ ] 多实例（`-multiInst`）时只有主实例写会话，其他实例不碰会话文件。

## 文件监控

- [ ] 监听文件本身会漏掉"临时文件 + 重命名"式保存 → 监听父目录。
- [ ] 一次保存会产生多个事件 → 去抖 200 ms。
- [ ] 自身保存触发通知 → 按 stamp 过滤。
- [ ] 缓冲区溢出 `ERROR_NOTIFY_ENUM_DIR` → 全量比对。
- [ ] 网络盘、某些虚拟文件系统不支持或不可靠 → 轮询兜底。
- [ ] 文件被删除：标记为"磁盘上已删除"，保留缓冲区，不要关闭文档。
- [ ] 窗口失去焦点期间的多次变化合并为一次提示，回到前台时再弹。

## 大文件 / mmap / Hex

- [ ] 映射期间外部截断 → 页访问抛 `EXCEPTION_IN_PAGE_ERROR` → SEH 包裹。
- [ ] 映射会阻止其他进程截断/轮转文件 → Tail 不用 mmap；大文件视图打开活跃日志时提示"建议用 Tail 模式"。
- [ ] 网络盘上的 mmap 性能差且断网即异常 → 网络路径改用分块 `ReadFile`。
- [ ] 行索引构建时 UI 不能等待 → 先按字节比例估算滚动条，索引逐步完善。
- [ ] 多字节编码的块边界可能切断字符 → 解码前对齐到字符边界（UTF-8 回退到起始字节；GB18030 从已知行首解码）。
- [ ] 32 位偏移溢出 → 一律 `u64`。

## 搜索

- [ ] PCRE2 对无效 UTF-8 输入会报错 → 编辑器内文本保证是 UTF-8；目录搜索对非 UTF-8 文件先转码或用 `PCRE2_MATCH_INVALID_UTF`。
- [ ] 灾难性回溯 → 设置匹配限制（`match_limit`/超时），超限时报告而不是卡死。
- [ ] 零长度匹配（如 `^`、`\b`）的全部替换要防止死循环。
- [ ] "全部替换"从后往前替换，或记录偏移增量。
- [ ] 搜索结果过多（几十万条）→ 列表用虚拟模式（`LVS_OWNERDATA`），设上限并提示。
- [ ] 目录搜索默认跳过 `.git`、`node_modules`、二进制和加密文件。

## 终端 / ConPTY

- [ ] `ClosePseudoConsole` 期间如果没人读输出管道会死锁 → 先保证读取线程在运行。
- [ ] 1809 自带 ConPTY 的已知问题（重绘、resize 闪烁、部分 VT 序列）→ 评估随附新版 conpty。
- [ ] 窗口大小变化要 `ResizePseudoConsole`，并做节流。
- [ ] 东亚宽字符占两格；emoji 宽度依赖 Unicode 版本 → 与 conhost 保持一致的宽度表。
- [ ] 自绘控件不处理 IME 就无法输入中文。
- [ ] Ctrl+C 语义冲突：有选区复制，无选区发中断。
- [ ] 关闭终端标签时 shell 的子进程（如正在跑的 python）要一并结束 → Job Object。

## Python 运行

- [ ] `python.exe` 可能是 `WindowsApps` 下的商店占位程序（0 字节重解析点），运行会弹商店 → 排除。
- [ ] 输出缓冲导致迟迟不显示 → `-u`。
- [ ] 中文乱码 → `PYTHONUTF8=1`、`PYTHONIOENCODING=utf-8`，按 UTF-8 解码输出。
- [ ] `input()` 在管道模式下阻塞 → 输出面板说明，提供"在终端中运行"。
- [ ] 停止只杀主进程会留下孤儿子进程 → Job Object。
- [ ] 读取管道必须两个线程分别读 stdout/stderr，否则一个管道写满会死锁。

## 侧边栏 / 系统图标

- [ ] `SHGetFileInfoW` 需要 COM 初始化；对网络路径会阻塞 → 用 `SHGFI_USEFILEATTRIBUTES` 只按扩展名取，必须读文件的在后台 STA 线程取。
- [ ] 系统图像列表要按 DPI 取合适尺寸（`SHGetImageList` + `SHIL_SMALL`/`SHIL_LARGE`）。
- [ ] TreeView 一次插入上万个节点很慢 → 懒加载，展开时才枚举。
- [ ] 工作区目录被删除/重命名 → 文件监控驱动刷新，不崩溃。

## 加密

- [ ] 忘记密码无法恢复 → 首次加密时明确警告，要求输入两次。
- [ ] nonce 绝不能重复 → 每次保存随机生成 12 字节。
- [ ] KDF 参数写入文件头，不要硬编码在读取代码中。
- [ ] 密钥用 `zeroize` 包裹，关闭文档时清除。
- [ ] 完整内存 dump 会包含明文 → 崩溃转储用 `MiniDumpNormal`。
- [ ] 目录搜索、Ctrl+P 预览等功能不得读取加密文件明文。
- [ ] 加密文件的会话清单中不要记录敏感预览（如首行作为标题）。

## Markdown 预览

- [ ] 每次按键都重新解析和排版会卡 → 防抖 150 ms，后台解析。
- [ ] 大图片解码阻塞 UI → WIC 解码放后台，缓存缩放后的位图。
- [ ] 相对图片路径以 Markdown 文件所在目录为基准。
- [ ] 链接点击用 `ShellExecuteW` 打开，只允许 `http(s)`、`mailto`、本地文件，不执行 `file:` 中的可执行文件。

## windows-sys

- [ ] 部分函数受多个特性控制：`CreateMutexW` 还需要 `Win32_Security`；`MessageBeep` 在 `Win32_System_Diagnostics_Debug` 而不是 `WindowsAndMessaging`。编译报"找不到"时先查特性。
- [ ] pcre2 crate 的 `disable_utf_check` 已废弃（新版 PCRE2 不再重复校验）。

## 发布

- [ ] CRT 静态链接（`-C target-feature=+crt-static`），否则未装 VC++ 运行库的机器无法启动。
- [ ] 第三方许可证汇总进 `THIRD_PARTY_NOTICES.txt`（Scintilla/Lexilla、PCRE2、alacritty_terminal 等），随包分发。
- [ ] 不签名 → 不做 Win11 新版右键菜单（需要签名的稀疏包），只写 HKCU 经典右键菜单（无需管理员）。
- [ ] 文件关联写 HKCU，不覆盖用户已有的默认程序，只加入"打开方式"列表。
