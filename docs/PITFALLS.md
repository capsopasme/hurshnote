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
- [x] Scintilla 在 `WM_CREATE` 里 `RegisterDragDrop`，需要已 `OleInitialize`；它的拖放目标不接受 CF_HDROP，初始化 OLE 后文件拖到编辑区会失效 → `hn_drop.cxx` 撤销它的注册，换成同时接受文件与文字的目标；文件路径交给回调后延后打开（拖放过程中不能弹对话框）。
- [x] 自写拖放目标处理编辑区内部"移动"文字：插入后要把选区放回原文字（位置按插入偏移），Scintilla 在 `DoDragDrop` 返回后删除选区，从而完成移动。
- [x] 两个视图共享文档时，`SCN_SAVEPOINTLEFT/REACHED`、`SC_UPDATE_CONTENT` 会从两个视图各发一次 → 按发出通知的视图找对应文档，非焦点视图只更新显示相关的状态。
- [x] `SCI_SETDOCPOINTER(0)` 会给视图新建一个空文档 → 取消分屏时用它释放第二个视图对文档的引用。
- [x] 激活文档可能把焦点移到另一视图，之前取到的 `sci` 就过期了 → 调用 `activate` 之后重新取 `self.sci()`。
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

- [ ] 快照必须在切换离开文档**之前**写：之后就读不到那个文档的文本了（Scintilla 只能读当前文档）。
- [ ] 删除旧快照必须排在"不再引用它的清单"写出之后，否则中途崩溃会留下引用不存在快照的清单。
- [ ] `SCN_UPDATEUI` 的 `SC_UPDATE_CONTENT` 在样式、标记变化时也会触发；用它标记"快照过期"只会多写一次，不影响正确性。
- [ ] 测试会话功能时用 `HURSHNOTE_DATA_DIR` 指向临时目录，不要污染真实会话。

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

## 大文件 / Hex / Tail

- [x] 映射期间外部截断 → 页访问抛 `EXCEPTION_IN_PAGE_ERROR` → **不用 mmap**，改为分块定位读，截断只会读到更少的字节。
- [x] 映射或长期持有的句柄会阻止轮转：映射阻止截断；即使以 `FILE_SHARE_DELETE` 打开，删除也会挂起到句柄关闭，期间无法新建同名文件 → 每次读取、每次轮询都"打开 → 读 → 关闭"。
- [x] 判断文件被替换不能用创建时间：Windows 文件系统隧道让 15 秒内同名重建的文件继承旧的创建时间 → 用文件索引号（`GetFileInformationByHandle`）。
- [x] 长度变大不一定是追加（可能被整体改写成更长的内容）→ 比较旧末尾 4 KB 的哈希。
- [x] 网络盘上的 mmap 性能差且断网即异常 → 同上，不用 mmap。
- [x] 行索引构建时 UI 不能等待 → 滚动条按字节比例，行号只在索引扫描到之后显示；跳转到行时提示索引进度。
- [x] 后台顺序扫描与界面的随机读取抢磁盘 → 索引线程设为后台 I/O 优先级（`THREAD_MODE_BACKGROUND_BEGIN`）。
- [x] 多字节编码的块边界可能切断字符 → 网格点对齐到字符边界（UTF-8 跳过续字节、UTF-16 跳过低代理项）；GB18030 等无法从任意位置判断边界，只在超过 2 KB 的超长行折行处可能错一个字符。
- [x] 超长单行（100 MB JSON）不能为了找行首向前扫描整行 → 超过 2048 字节的行按绝对偏移网格折成多个显示行，任何位置只需扫描不超过 4 KB。
- [x] UTF-16 的 LF 是偶数偏移上的 `0A 00`（LE）；奇数偏移上的 `00 0A` 不是换行。
- [x] 32 位偏移溢出 → 一律 `u64`；滚动条范围固定 0..65535，按比例映射。
- [x] 被盖住的 Scintilla 仍会收到焦点（`SetFocus(sci)` 散布在各处）→ 子类化 `WM_SETFOCUS` 转给大文件视图，否则键入会进入隐藏的文档。
- [x] `SetScrollInfo` 改变滚动条显隐时会同步发 `WM_SIZE` → 用 `SIF_DISABLENOSCROLL` 让滚动条常驻，并且调用前释放 `LargeDoc` 的借用。
- [x] 大文件视图的焦点变化不能同步切换编辑视图（会在 `sync_overlays` 中途重入）→ `PostMessage(WM_APP_FOCUS_VIEW)`。
- [x] 中英文混排时 DirectWrite 每行高度、基线不同 → 文本格式设统一行距（用含中文的样本测出行高与基线）。
- [x] CI 刚写完 5 GB 文件就测随机读，后台写回把延迟拖到几百毫秒 → 生成后先 fsync。

## 搜索

- [x] PCRE2 对无效 UTF-8 输入会报错 → 编辑器内文本保证是 UTF-8；目录搜索对每个文件先嗅探编码、转成 UTF-8 再匹配。
- [ ] 灾难性回溯 → 设置匹配限制（`match_limit`/超时），超限时报告而不是卡死。
- [ ] 零长度匹配（如 `^`、`\b`）的全部替换要防止死循环。
- [ ] "全部替换"从后往前替换，或记录偏移增量。
- [x] 搜索结果过多（几十万条）→ 列表用虚拟模式（`LVS_OWNERDATA`），设上限并提示（目录搜索每个文件最多记录 1 万处）。
- [x] 目录搜索默认跳过 `.git`（隐藏目录）、`node_modules`、二进制和加密文件。
- [x] 只有一个 NUL 的小二进制样本会被 UTF-16 启发式当成 UTF-16BE（NUL 全在偶数位）→ 测试样本要让 NUL 在奇偶位置上都出现；真实二进制文件一般不受影响。
- [x] 遍历得到的路径可能含 8.3 短名（CI 的临时目录是 `C:\Users\RUNNER~1\...`），而文档路径是规范化的长路径 → 判断"文件是否已打开"前先 `canonicalize`。
- [x] 目录替换不能把有损解码的文本写回（会把无法识别的字节变成 U+FFFD）→ 有损或替换后无法用原编码表示时跳过并说明原因。
- [x] 搜索线程每批都 `PostMessage` 会灌满消息队列 → `pending` 标志：只有 UI 处理过上一条唤醒后才再投递；结束消息投递前先清标志，保证一定会被处理。

## 终端 / ConPTY

- [x] `ClosePseudoConsole` 期间如果没人读输出管道会死锁 → 关闭终端只发 `Shutdown`，PTY 在 alacritty 的事件循环线程里释放，UI 线程不等待。
- [ ] 1809 自带 ConPTY 的已知问题（重绘、resize 闪烁、部分 VT 序列）→ 评估随附新版 conpty。
- [x] 窗口大小变化要 `ResizePseudoConsole` → 按格子算出的行列数没变时不发送。
- [ ] 东亚宽字符占两格；emoji 宽度依赖 Unicode 版本 → 与 conhost 保持一致的宽度表。
- [x] 自绘控件不处理 IME 就无法输入中文 → 组字窗口用 `ImmSetCompositionWindow` 跟随光标，其余交给 `DefWindowProc`，上屏文字以 `WM_CHAR` 到达。
- [x] 加速键表会吃掉终端里的 Ctrl+C/V/A/F、F5 等 → 焦点在终端时只让 Ctrl+` / Ctrl+Shift+` 走加速键。
- [x] Alt+字母在终端里会激活菜单栏 → 终端自己处理 `WM_SYSCHAR`（发 ESC 前缀），只有 Alt+空格、Alt+F4 交给 `DefWindowProc`。
- [x] Backspace 产生的 `WM_CHAR` 是 0x08，但大多数程序期望 DEL（0x7f）→ 互换；Shift+Tab、Ctrl+空格在 `WM_KEYDOWN` 中处理后要丢掉紧随的 `WM_CHAR`。
- [x] PTY 输出很多时每次唤醒都 `PostMessage` 会灌满队列 → `pending` 标志合并唤醒，绘制时一次取快照。
- [x] 回退字体（中文、符号）的前进宽度和等宽网格不一致，整行一起画会把后面的字符挤歪 → 非 ASCII 字符逐个画在自己的格子里。
- [x] Ctrl+C 语义冲突：有选区复制，无选区发中断。
- [x] 关闭终端标签时 shell 的子进程（如正在跑的 python）要一并结束 → 关闭 ConPTY 时其中的进程随之结束。

## Python 运行

- [x] `python.exe` 可能是 `WindowsApps` 下的商店占位程序（0 字节重解析点），运行会弹商店 → 排除。
- [x] 输出缓冲导致迟迟不显示 → `-u`。
- [x] 中文乱码 → `PYTHONUTF8=1`、`PYTHONIOENCODING=utf-8`，按 UTF-8 解码输出。
- [x] `input()` 在管道模式下阻塞 → 输出面板说明，提供"在终端中运行"。
- [x] 停止只杀主进程会留下孤儿子进程 → Job Object。
- [x] 读取管道必须两个线程分别读 stdout/stderr，否则一个管道写满会死锁。
- [x] Job Object 设了 `KILL_ON_JOB_CLOSE`，`Running` 一丢弃进程就被结束（测试里 `spawn(..).unwrap();` 直接丢弃，输出为空）→ 运行期间必须持有 `Running`。
- [x] "退出"事件可能先于最后一段输出到达 → 等待线程在两个读取线程结束后才发"退出"。
- [x] 输出块可能在 UTF-8 字符或转义序列中间断开 → 每个流一个增量解码器。
- [x] Rust 的 `Command` 会给含空格的参数加引号，`cmd /c "echo a& echo b"` 中 cmd 会把整串当成命令名 → 给 cmd 传命令时每段参数不含空格，或改用 `raw_arg`。

## 侧边栏 / 系统图标

- [x] `SHGetFileInfoW` 需要 COM 初始化；对网络路径会阻塞 → 用 `SHGFI_USEFILEATTRIBUTES` 只按扩展名取（`OleInitialize` 已初始化 COM）。
- [ ] 系统图像列表要按 DPI 取合适尺寸（`SHGetImageList` + `SHIL_SMALL`/`SHIL_LARGE`）。目前用 `SHGFI_SMALLICON`，高 DPI 下图标偏小。
- [x] TreeView 一次插入上万个节点很慢 → 懒加载，展开时才枚举，单个目录最多 1 万项。
- [x] 工作区目录被删除/重命名 → 回到前台时同步，路径不存在的节点由上一级删除，不崩溃。
- [x] 系统图像列表是共享的，不能销毁 → ListView 用 `LVS_SHAREIMAGELISTS`；TreeView 本来就不销毁 `TVSIL_NORMAL`。
- [x] `TVINSERTSTRUCTW` 里的 item 是 `TVITEMEXW` / `TVITEMW` 的联合，自己声明时按较大的 `TVITEMEXW` 补齐。
- [x] `NMLVKEYDOWN` / `NMTVKEYDOWN` 在 commctrl.h 中按 1 字节对齐（`#include <pshpack1.h>`），Rust 声明要 `#[repr(C, packed(1))]`。
- [x] 虚拟 ListView 程序设置选中项也会发 `LVN_ITEMCHANGED` → 用一个"抑制"标志区分用户操作；`LVM_SETITEMCOUNT` 传 `LVSICF_NOSCROLL` 避免每次刷新都滚动。
- [x] TreeView 的 `NM_RCLICK` 返回非零才不会再发 `WM_CONTEXTMENU`；ListView 的右键菜单改在 `WM_CONTEXTMENU` 里做（键盘 Shift+F10 时 `lParam == -1`，用选中项）。
- [x] 加速键表在 `TranslateAcceleratorW` 阶段就吃掉按键 → 焦点在侧边栏或快速打开时，不带 Ctrl 的键跳过加速键（F2 重命名、Del 删除、F5 刷新）。
- [x] Ctrl+Tab 切换器要在 Ctrl 松开时提交，而 `WM_KEYUP` 发给的是焦点窗口（编辑器）→ 在消息循环里统一预处理；切换列表用 `WS_EX_NOACTIVATE` 弹出，焦点留在编辑器。

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

- [ ] `SHELLEXECUTEINFOW` 需要 `Win32_System_Registry` 特性（字段含 HKEY）；`CreateFileW` 需要 `Win32_Security`。
- [ ] `SetUnhandledExceptionFilter` 与 `MiniDumpWriteDump` 的类型牵涉大量特性，直接自己声明（kernel32 链接 + dbghelp 动态加载）更省事。

- [x] windows 0.58：`ID2D1RenderTarget::CreateSolidColorBrush` 需要 `Foundation_Numerics` 特性（画刷属性含 Matrix3x2），其他常用绘制方法只要 `Win32_Graphics_Direct2D_Common`。
- [x] windows 0.58 的 `HWND` 是 `HWND(*mut c_void)`，windows-sys 0.59 是 `*mut c_void`，跨 crate 传递时用 `HWND(raw)` 包装。
- [x] `Option<&dyn Fn(&[u8]) -> String>` 会触发 clippy `type_complexity` → 用类型别名（`hn_large::decode::LegacyDecode`）。
- [x] 不要在自测函数里把两个 crate 的同名类型（`hn_search::Mode` 与 `lview::Mode`）同时 `use` 进来。
- [ ] 部分函数受多个特性控制：`CreateMutexW` 还需要 `Win32_Security`；`MessageBeep` 在 `Win32_System_Diagnostics_Debug` 而不是 `WindowsAndMessaging`。编译报"找不到"时先查特性。
- [ ] pcre2 crate 的 `disable_utf_check` 已废弃（新版 PCRE2 不再重复校验）。
- [x] `SHBrowseForFolderW`、`BROWSEINFOW` 需要 `Win32_UI_Shell_Common`（`ITEMIDLIST`）；`SystemParametersInfoForDpi` 在 `Win32_UI_HiDpi`；`CoTaskMemFree` 在 `Win32_System_Com`；`OleInitialize` 在 `Win32_System_Ole`。
- [x] 自己声明的 Win32 结构体名（`NMHDR`、`TVITEMW`……）会触发 clippy `upper_case_acronyms` → 模块级 `allow`。
- [x] `unsafe { f() } < 0` 会被解析成泛型参数 → 先绑定到变量再比较。

## 发布

- [ ] CRT 静态链接（`-C target-feature=+crt-static`），否则未装 VC++ 运行库的机器无法启动。
- [ ] 第三方许可证汇总进 `THIRD_PARTY_NOTICES.txt`（Scintilla/Lexilla、PCRE2、alacritty_terminal 等），随包分发。
- [ ] 不签名 → 不做 Win11 新版右键菜单（需要签名的稀疏包），只写 HKCU 经典右键菜单（无需管理员）。
- [ ] 文件关联写 HKCU，不覆盖用户已有的默认程序，只加入"打开方式"列表。
