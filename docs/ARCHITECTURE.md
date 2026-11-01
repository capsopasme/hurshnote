# HurshNote 架构设计

> 配套文档：[REQUIREMENTS.md](REQUIREMENTS.md)（需求与取舍）、[PLAN.md](PLAN.md)（里程碑）、[PITFALLS.md](PITFALLS.md)（坑清单）

## 1. 总体思路

**编辑内核用成熟的 Scintilla，外壳和所有周边能力用 Rust 自己写。**

Notepad++ 的路线（原生 Win32 + Scintilla）启动快、体积小、中文输入稳，这一点保留。它的短板在大文件（全量读入内存）、
文件 IO 可靠性、外壳交互和配套工具，这些全部重写。

```
┌──────────────────────────── hurshnote.exe（单个可执行文件）────────────────────────────┐
│  UI 线程（唯一操作窗口的线程）                                                          │
│  ┌──────────┐ ┌───────────────────────────────┐ ┌───────────────┐                      │
│  │ 侧边栏    │ │ 编辑区                         │ │ MD 预览（D2D）│                      │
│  │ TreeView │ │ Scintilla × N 视图             │ │               │                      │
│  │ 系统图标  │ │ 或 大文件视图 / Hex 视图（D2D）│ │               │                      │
│  └──────────┘ └───────────────────────────────┘ └───────────────┘                      │
│  ┌────────────────── 底部抽屉：输出 | 终端 | 搜索结果 ──────────────────┐               │
│  └──────────────────────────────────────────────────────────────────────┘               │
│  状态栏：行列 | 选区 | 编码 | EOL | 只读 | Dirty                                        │
├────────────────────────────────────────────────────────────────────────────────────────┤
│  后台线程池：文件加载/解码、快照写入、目录搜索、行索引、文件监控、图标提取、PTY 读写   │
│         └── 结果经 channel 回传 + PostMessage(WM_APP_*) 唤醒 UI 线程                    │
└────────────────────────────────────────────────────────────────────────────────────────┘
```

## 2. 技术栈

| 层 | 选择 | 备注 |
|---|---|---|
| 语言 | Rust stable，`x86_64-pc-windows-msvc`，`rust-toolchain.toml` 固定版本 | Rust 的 MSVC 目标最低要求 Win10，与 1809 底线一致 |
| Win32 绑定 | `windows-sys` 0.59（普通 Win32 API，零开销、签名稳定）；COM（Direct2D / DirectWrite，以后的 WIC）用 `windows` 0.58，只在 `hn-render` 中使用 | 不引入任何 GUI 框架 |
| 编辑内核 | Scintilla 5.x + Lexilla 5.x，源码放在 `third_party/`，用 `cc` crate 以 C++17 编译并**静态链接** | 静态时调用 `Scintilla_RegisterClasses`；lexer 用 `CreateLexer` + `SCI_SETILEXER` |
| 文本渲染 | Scintilla 用 `SC_TECHNOLOGY_DIRECTWRITE`；自绘视图用 Direct2D + DirectWrite | 高 DPI、彩色 emoji、字体回退 |
| 编码 | `encoding_rs` + `chardetng` | GB18030 往返无损 |
| 正则 | `pcre2`（JIT），编辑器内查找与目录搜索共用 | 支持环视、反向引用，行为与 N++ 用户习惯接近 |
| 目录搜索 | `ignore`（并行遍历 + gitignore）+ `grep-searcher` + `grep-pcre2` | ripgrep 的组件 |
| 大文件 | `hn-large`（只依赖 std）：分块定位读 + 64 KiB 块缓存，**不用内存映射** | 理由见 §6.4 |
| 终端 | ConPTY 薄封装（自写，参考 `portable-pty`）+ `alacritty_terminal`（VT 状态机）+ 自写 D2D 网格渲染 | |
| Markdown | `pulldown-cmark`（GFM 表格/任务列表/删除线）+ 自写布局与 D2D 绘制 | |
| 加密 | `argon2` + `aes-gcm` + `zeroize`，随机数来自 `getrandom`（BCryptGenRandom） | |
| 配置 | `serde` + `toml` | |
| 日志 | `tracing` + 滚动文件 | 默认只记录 warn 以上 |
| 构建 | Cargo workspace；GitHub Actions `windows-latest` 产出 exe；CRT 静态链接（`+crt-static`） | 第三方 C++ 源码由 `vendor` 工作流从官网下载并提交到 `third_party/` |
| 打包 | 便携 zip + Inno Setup（不签名） | |

## 3. Crate 划分

```
hurshnote/
  Cargo.toml                 workspace
  rust-toolchain.toml
  third_party/
    scintilla/  lexilla/     上游源码原样放入，记录版本号，不改源码（必要补丁放 patches/）
  crates/
    hn-app       Win32 外壳：主窗口、布局、菜单、快捷键、侧边栏、状态栏、对话框、单实例、命令行
    hn-sci       Scintilla/Lexilla 编译脚本 + 类型安全封装 + 主题映射表
    hn-core      文档模型、文档管理器、保存流程、会话/快照、文件监控
    hn-text      编码嗅探与转换、EOL 检测与转换、行操作、列表续写、转义解析   ← 纯函数，跨平台可测
    hn-large     分块读取与块缓存、稀疏行索引、显示行、分块遍历、Hex、Tail    ← 只依赖 std，跨平台可测
    hn-search    查找替换引擎、目录搜索                                         ← 跨平台可测
    hn-crypto    加密文件格式读写                                               ← 跨平台可测
    hn-md        Markdown → 布局树（与平台无关）；绘制在 hn-app
    hn-term      ConPTY 封装 + VT 状态 + 输入映射
    hn-run       Python 解释器探测、进程启动、Job Object、输出流
    hn-render    共享的 D2D/DWrite 设施：字体、颜色、文本布局缓存（Hex、大文件、终端、MD 共用）
  tests/corpus/  测试语料（编码、换行、超长行、加密样本……）
  docs/
```

**M5 新增：** `hn-run`（解释器探测、子进程与 Job Object、Traceback 定位、ANSI 颜色解码；只依赖 std）、`hn-term`（`alacritty_terminal` 封装：PTY、VT 状态机、渲染快照、按键编码、配色），`hn-search::dir`（目录搜索与替换核心），`hn-app` 中的 `term`（终端窗口）、`dirdlg`（在文件中查找对话框），以及 `app` 的子模块 `drawer`（底部抽屉：页签、搜索结果、输出面板、终端页）、`dirsearch`（目录搜索与替换）、`runpy`（运行 Python）。M4 的搜索结果面板并入抽屉。

**M4 新增：** `hn-large`（字节源与块缓存、稀疏行索引、显示行导航、按行对齐的分块遍历、Hex 行格式、文件状态与变化分类；只依赖 std，Linux 上有完整单元测试）、`hn-render`（Direct2D/DirectWrite 封装），`hn-app` 中的 `lview`（大文件视图窗口与文档状态）以及 `app` 的子模块 `large`（打开、轮询、Tail、Hex 切换、后台搜索、搜索结果面板、状态栏）。`hn-term`、`hn-run`、`hn-md` 尚未创建。

**M2 新增：** `hn-crypto`（加密格式）、`hn-core`（会话清单、快照、文档数据布局、磁盘戳、原子写入；均跨平台），`hn-app` 中的 `fsio`（安全保存与提权）、`watcher`、`crashdump`、`pwdlg`，以及 `app` 的子模块 `persist`（快照与会话）、`files`（保存、加密文件、外部修改）。

**M1 实际的 `hn-app` 模块：** `app`（窗口、状态、命令分发）、`docs`（文档模型与加载）、`commands`（命令 ID、快捷键表、菜单）、`statusbar`、`finddlg`（查找/替换、跳转对话框）、`dlg`（内存对话框模板）、`editops`（行操作、书签、列表续写、括号匹配）、`config`（设置与最近文件）、`instance`（命令行与单实例）、`selftest`。`hn-core` 尚未拆出，M2 实现安全保存与快照时再从 `app`/`docs` 中拆分。

**原则：**
- 能不依赖 Win32 的逻辑都放在跨平台 crate 里，这样在 Linux 上也能跑单元测试、模糊测试。
- `hn-app` 只做"把事件翻译成调用"和"把状态画出来"，不写业务逻辑。
- 只有 `hn-sci` 能直接 `SendMessage` 给 Scintilla，其他模块通过它的封装访问。

## 4. 线程模型（第一天定死，不允许例外）

1. **所有窗口（包括 Scintilla）只在 UI 线程创建和操作。** 后台线程绝不 `SendMessage` 给任何窗口。
2. 后台任务通过 `crossbeam-channel` 把结果发回，并 `PostMessage(hwnd_main, WM_APP_TASK, 0, 0)` 唤醒；UI 线程在消息处理里排空 channel。
3. 高频结果（搜索命中、终端输出、Python 输出）**合批**：最多每 16–50 ms 唤醒一次 UI。
4. 每个后台任务带一个 `CancelToken`；文档关闭、重新搜索时取消旧任务。
5. 需要 COM 的后台线程（图标提取）单独初始化为 STA。
6. 大段文本从 Scintilla 取出必须在 UI 线程：`SCI_GETCHARACTERPOINTER` 拿到连续 UTF-8 后 `memcpy`（100 MB 约几十毫秒），再交给后台。

## 5. 核心数据模型

```rust
// hn-core（示意，非最终代码）
struct Document {
    id: DocId,
    path: Option<PathBuf>,          // None = 未命名
    kind: DocKind,                  // Editable(SciDocPtr) | Large(LargeFile)
    encoding: TextEncoding,         // Utf8 { bom } | Utf16Le { bom } | Utf16Be { bom } | Gb18030 | Other(&'static Encoding)
    encoding_lossy: bool,           // 解码时出现无法映射的字节
    eol: EolMode,                   // Crlf | Lf | Cr | Mixed
    language: LangId,
    read_only: bool,
    encrypted: Option<CryptoState>, // 持有派生密钥（zeroize）、salt、KDF 参数
    disk_stamp: Option<DiskStamp>,  // 加载/保存时的 mtime + size + 内容哈希（xxh3）
    snapshot: Option<SnapshotId>,
}
```

- **视图与文档分离**：一个 Scintilla 文档（`SCI_CREATEDOCUMENT` 得到的指针）可挂到多个 Scintilla 窗口（`SCI_SETDOCPOINTER`），这就是 Clone View。文档引用计数由 `hn-sci` 管理（`SCI_ADDREFDOCUMENT` / `SCI_RELEASEDOCUMENT`）。
- **懒加载**：恢复会话时只加载当前可见的文档，其余文档在被激活时才读盘。20 个文件的会话启动时间与 1 个文件相同。
- **后台加载**：用 Scintilla 的 `SCI_CREATELOADER` 得到 `ILoader`，在后台线程读文件、解码、`AddData`，完成后在 UI 线程 `ConvertToDocument` 并挂到视图。打开 100 MB 文件时界面不冻结。
- Scintilla 内部统一使用 UTF-8（`SC_CP_UTF8`），编码转换只发生在读和写两个边界。

## 6. 关键子系统设计

### 6.1 打开文件的决策流程

```
stat(file)
 ├─ 魔数 = HURSHENC ──────────────────────────────→ 输入密码 → 解密 → 编辑器
 ├─ size > large_threshold (256MB) ───────────────→ 大文件视图（只读）
 ├─ 前 8KB 含 NUL 且不像 UTF-16 ──────────────────→ 提示：是 = 以 Hex 模式的大文件视图打开 / 否 = 按文本打开 / 取消
 └─ 其他 → 后台读取 + 编码嗅探 + 解码（ILoader）
             └─ 最长行 > long_line_threshold (默认 1 MB) → 关闭折行、lexer、缩进线，状态栏提示
```

### 6.2 编码嗅探（hn-text）

顺序：用户对该路径的记忆选择 → BOM → UTF-16 无 BOM 启发式（偶/奇位置 NUL 比例）→ 严格 UTF-8 校验通过即 UTF-8
→ `chardetng`（TLD 提示按系统区域，中文系统给 `cn`）→ 系统 ANSI 代码页。

- 解码有替换字符时设置 `encoding_lossy = true`，保存前弹窗警告（"按当前编码保存会损坏 N 处字节"）。
- 纯 ASCII 文件标记为 UTF-8（无 BOM），不显示为 ANSI。
- "以某编码重新打开"（重新解码磁盘字节）和"转换为某编码"（保持文字、改变保存编码）是两个不同命令，必须分开。

### 6.3 保存（hn-core）

```
1. 取出 UTF-8 文本（UI 线程拷贝）→ 后台：EOL 统一 → 编码（+BOM）→ 可选加密
2. 目标是硬链接（链接数 > 1）或符号链接/联接点的目标？
     是 → 先备份到 backup 目录，再原地截断写入
     否 → 写同目录临时文件 → FlushFileBuffers → ReplaceFileW(target, tmp)
             ReplaceFileW 失败（跨卷、网络盘、对方未开 FILE_SHARE_DELETE）→ 退化为备份 + 原地写
3. ERROR_ACCESS_DENIED 且目标受保护 → 询问后 ShellExecuteEx("runas") 启动
     `hurshnote.exe --elevated-save <tmp> <target>`，辅助进程只做复制
4. 更新 disk_stamp；通知文件监控忽略这次自身写入；删除该文档的快照
```

### 6.4 大文件视图（hn-large + hn-app/lview + app/large）

**只读**。打开时只读开头 256 KB 嗅探编码与换行符、文件末尾 4 KB 记录文件状态，然后立即显示第一屏；行索引在后台建立。

**数据访问（`hn_large::source`）：不使用内存映射。** `FileSource` 每次读取都"打开 → 定位读 → 关闭"（Windows 上 std 以读/写/删除共享模式打开），UI 线程通过 `Reader` 读取，按 64 KiB 块缓存最近 128 块（8 MiB）。原计划的 mmap + SEH 方案放弃，原因：
- 映射存在期间，写日志的程序无法截断或轮转文件（`ERROR_USER_MAPPED_FILE`）；长期持有的句柄会让"删除后重建"式轮转失败（删除挂起期间不能新建同名文件）
- 文件被外部截断后访问映射页会触发 `EXCEPTION_IN_PAGE_ERROR`，需要 C 写的 SEH 包装；定位读只会读到更少的字节
- 网络盘上 mmap 性能差、断网即异常
- 视图每次只需要几十 KB，顺序扫描用 4 MiB 的块读，与 mmap 没有可感知的差距（CI 上 5 GB 日志约 5 秒建完索引）

**行索引（`hn_large::index`）**：按文件绝对偏移每 64 KiB 一块，记录"到该块末尾为止的 LF 总数"（5 GB 约 8 万项、640 KB）。偏移 → 行号、行号 → 偏移都只需在索引之外再扫描不超过一块。后台线程顺序读文件、每 100 ms 发一批结果（以后台 I/O 优先级运行，不和界面读取抢磁盘）；文件增长时从最后一个完整块继续。行以 LF 结尾（CRLF 视为一个换行），单独的 CR 不算换行；UTF-16 的 LF 必须位于偶数偏移。

**显示行（`hn_large::rows`）**：以"显示行"为单位滚动。不超过 2048 字节的行占一个显示行；更长的行在按绝对偏移划分的网格点（`data_start + k × 2048`，再对齐到字符边界：跳过 UTF-8 续字节、UTF-16 低代理项）处折开。任意偏移所在显示行的起点只需向前、向后各扫描不超过 4 KB 就能确定，与索引是否建好无关，100 MB 的单行 JSON 也不会卡。随机测试检查"从头遍历每一行、任意位置求行首、反向求上一行"三者一致。

**滚动**：顶部位置是一个字节偏移（显示行起点）。滚动条按字节比例映射（0..65535），拖动时按比例定位后对齐到显示行起点；最后一行最多滚到窗口底部。行号栏显示逻辑行号：顶部行号 = 索引中该偏移之前的 LF 数（索引未扫描到时不显示行号）。

**绘制（`lview`）**：每个编辑视图上叠放一个自绘子窗口，显示该视图文档的 `LargeDoc` 时盖住 Scintilla（Scintilla 中挂一个空的只读文档占位，焦点经子类化转给大文件视图）。每帧只排版可见的显示行：读字节 → `hn_large::decode` 解码为 UTF-16 并记录每个码元的源字节偏移 → DirectWrite 单行排版；命中测试与选区高亮都通过偏移表在字节与码元之间换算。控制字符显示为 Unicode 控制图形。

**编码**：嗅探与普通文档相同（`hn_text::encoding::detect`）；按编码族（UTF-8 / UTF-16 LE/BE / GB18030 / 双字节 / Shift_JIS / EUC-JP / 单字节）判断字符边界。所有 ASCII 兼容编码中 0x0A 都不会出现在多字节字符内部，因此按字节找 LF 是安全的。"重新打开为"改变解读方式并重建索引。

**选中与复制**：光标、选区都是字节偏移，键盘/鼠标操作与普通编辑器一致（方向键、PgUp/PgDn、Home/End、Ctrl+Home/End、Shift 扩展、双击选词、拖动时自动滚动）。复制按当前编码解码，上限 64 MB。

**搜索（`app/large` + `hn_large::search`）**：后台线程按约 4 MiB、切在换行之后的块流式匹配（正向/反向两种遍历）。UTF-8 块整块交给 PCRE2；其他编码或含无效字节的块逐行解码（带偏移表）后匹配，再把位置换回源字节偏移。查找下一个/上一个（可绕回）、计数、"标记全部"（结果进搜索结果面板，最多列 10 万条，计数不受限）；Esc 取消；新搜索取消旧搜索；结果按代号丢弃过期批次。跨行匹配不保证（块边界处）。

**搜索结果面板**：主窗口底部的虚拟 ListView（行号 | 内容预览），双击或回车跳转，F7 显示/隐藏，Esc 隐藏。M5 的底部抽屉会接管它。

**Tail 与文件变化（`hn_large::tail`）**：每 0.5 秒对每个大文件"打开 → 取文件标识（Windows 文件索引号）、长度、修改时间与末尾 4 KB 哈希 → 关闭"：
- 长度变大且旧末尾哈希不变：增长 → 失效最后一个缓存块、从最后一个完整块继续建索引；Tail 时光标与视图跟到末尾；非 Tail 时提示一次"可开启 Tail"
- 文件标识变化：被替换（轮转）；长度变小：被截断；长度或修改时间变化但旧末尾哈希不同：被改写 → 清空缓存与索引、重新读取，提示用户
- 不存在：标记"磁盘上已删除"，重新出现时按"被替换"处理

不用创建时间判断替换：Windows 的文件系统隧道会让 15 秒内同名重建的文件继承旧的创建时间。

普通文档（已保存、未修改、未加密）开启 Tail 时改用大文件视图显示（Scintilla 内容设为只读保留）；关闭 Tail 且文件不超过阈值时回到编辑器并重新载入。

**会话**：大文件文档在清单中记 `large`、`hex`、`tail`、`offset`（光标字节偏移），恢复时同样懒加载；不写快照。大文件文档不加入文件监控（监控需要读全文算哈希），由上面的轮询负责。

### 6.5 Hex 视图

- 数据源 `hn_large::ByteSource`：`FileSource`（磁盘文件，大文件文档）、`MemSource`（普通文档按当前编码编码后的字节快照）。
- 固定 16 字节一行、虚拟滚动，只读取可见范围。列：偏移（至少 8 位十六进制，按文件长度加宽）| 16 字节 Hex（第 8 字节后多一个空格）| 可打印 ASCII（其他字节显示为 `.`，保证列对齐）。
- 大文件文档在文本/Hex 两种模式间切换（Ctrl+Shift+H），共用同一个光标字节偏移，切换后定位不丢失。
- 普通文档按 Ctrl+Shift+H 生成编码后的快照（含 BOM）显示；光标位置按"编码光标之前的文本"的长度换算，切回时按"解码光标之前的字节"的长度换回。快照显示期间编辑类命令被拦截；文档被重新载入时快照作废。
- Hex 模式下复制得到空格分隔的十六进制字节。v1 只读；覆盖式 Hex 编辑放到 v1 之后。

### 6.6 快照与会话恢复（hn-core）

位置：`%APPDATA%\HurshNote\`（便携模式为 exe 同目录 `data\`；测试时可用环境变量 `HURSHNOTE_DATA_DIR` 指定）

```
session.json              会话清单（hn-core::session，JSON，字段均有默认值）
snapshots/<随机>.snap      每个有未保存内容的文档一个快照（hn-core::snapshot：HNSNAP01 + 类型 + 内容）
backup/                   原地写入前的临时备份（写成功后删除）
running.marker            运行标记：启动时创建、正常退出时删除，启动时已存在说明上次异常退出
settings.ini / recent.txt 设置与最近文件
```

实现位于 `hn-app/src/persist.rs`（`app` 的子模块）。快照内容与加密文档明文共用 `hn-core::payload` 布局（`HNDOC1` + `键=值` 元数据 + 文本）。

- 触发：每次修改后防抖 1.5 s；连续编辑时最长 10 s 强制一次；切换文档、失去焦点、关闭程序时立即一次。
- 写入：后台线程写 `.snap.tmp` → `FlushFileBuffers` → `MoveFileExW(REPLACE_EXISTING | WRITE_THROUGH)`。**先写快照，最后写清单**，任何时刻崩溃都只会丢最后不到 1.5 s 的输入。
- 清单字段：路径、快照 id、编码、BOM、EOL、语言、只读、光标/选区/滚动位置、折叠状态、书签行、加载时的 `disk_stamp`。
- 恢复：快照存在 → 用快照内容并标记 Dirty；同时比对磁盘 `disk_stamp`，不一致时提示"磁盘文件在程序关闭期间被修改"，提供"保留我的修改 / 载入磁盘版本 / 对比"。
- 关闭程序时**不弹"是否保存"**（与 N++ 的快照模式一致），可在设置里改回传统行为。
- 撤销历史 v1 不持久化（新版 Scintilla 提供撤销历史读写接口，后续评估）。
- 恢复的快照文档在 Scintilla 中是"未修改"状态（载入时设了保存点），用 `meta_dirty` 标记为需要保存。
- 非当前文档的书签、折叠、换行符、只读状态只能在它是当前文档时读取，因此在切换离开时记录到 `Doc` 中。

### 6.7 文件监控（hn-app/watcher）

**M2 实现采用轮询，而不是 ReadDirectoryChangesW：**
- 打开的文件通常只有几十个，后台线程每秒 `stat` 一次（修改时间 + 大小）开销可以忽略。
- 天然支持网络盘与"写临时文件再改名"式保存；没有通知缓冲区溢出、监听句柄占用目录（阻止重命名/删除目录）的问题。
- 后台线程只报告"元数据变了"，UI 线程读取文件计算哈希，与文档的 `disk` 戳比较：内容相同（自己保存、外部 touch）则只更新戳。
- 未修改的文档默认静默重新载入（可关），已修改的询问；不重新载入时记住新戳，不再为同一版本提示。
- 文件被删除：标题显示"磁盘上已删除"，保留缓冲区。
- 窗口不在前台时事件先积累，`WM_ACTIVATE` 后再处理。
- 大文件文档不在这里监控（算哈希要读整个文件），改由大文件视图每 0.5 秒轮询（§6.4）。

### 6.8 查找替换（hn-search）

- 编辑器内：`SCI_GETCHARACTERPOINTER` 拿连续缓冲区 → PCRE2 匹配 → `SCI_SETSEL` / `SCI_REPLACETARGET`。全部替换在一个 `SCI_BEGINUNDOACTION` / `SCI_ENDUNDOACTION` 里完成，并且从后往前替换以避免偏移失效。
- 三种模式：普通 / 转义（`\n \r \t \0 \xNN \uNNNN`）/ 正则；选项：大小写、全词、仅选区、循环、方向。
- 目录搜索（`hn_search::dir`）：后台 `ignore::WalkParallel`（遵守 `.gitignore`、跳过隐藏文件、包含/排除通配符，默认排除 `node_modules`）；每个文件读入后用 `hn-text` 嗅探编码、解码为 UTF-8 再用 PCRE2 匹配，所以 GBK、UTF-16 文件也能搜，结果的列号是解码后行内的 UTF-8 字节偏移（与编辑器缓冲区一致）；跳过二进制文件、加密文件（魔数）、超过 64 MB 的文件；每个文件最多记录 1 万处。结果按文件合批，约每 100 ms 经 channel + `PostMessage(WM_APP_DIRSEARCH)` 送 UI（`pending` 标志合并唤醒，代号丢弃过期结果）；`AtomicBool` 取消。结束后按路径排序重排列表。
- 在目录中替换：先按同样条件搜索，结果列表即预览，结束后确认；已在编辑器中打开的文件在缓冲区内替换（一个撤销组，需要用户保存）；其余文件按原编码（含 BOM）重新编码后走 `fsio::save_file` 安全写入。解码有损、或替换后有字符无法用原编码表示的文件跳过并在输出面板列出原因。

### 6.9 加密文件格式 v1（hn-crypto）

```
偏移  长度  字段
0     8     magic = "HURSHENC"
8     2     format_version = 1 (LE)
10    1     kdf_id = 1 (Argon2id)
11    4     argon2_m_cost_kib (LE)   默认 65536（64 MiB）
15    4     argon2_t_cost (LE)       默认 3
19    1     argon2_p_cost            默认 4
20    16    salt
36    1     cipher_id = 1 (AES-256-GCM)
37    12    nonce
49    ...   ciphertext || 16 字节 tag
```

- 整个头部（0..49）作为 AEAD 的附加认证数据（AAD），任何字段被篡改都解密失败。
- 明文 = 小型元数据（原编码、EOL、语言）+ UTF-8 文本。
- 密钥只在内存中保存派生结果，`zeroize` 包裹；**每次保存生成新的随机 nonce**，salt 只在修改密码时更新（因此保存不需要重新跑 KDF）。
- 默认 KDF 参数：64 MiB、3 轮、单通道（argon2 crate 单线程计算多通道也不会更快）。读取时拒绝超过 1 GiB / 64 轮 / 64 通道的参数，防止恶意文件耗尽资源。
- KDF 参数写在文件头里，以后提高默认参数不影响旧文件打开。
- 错误密码与文件损坏统一报"密码错误或文件已损坏"（GCM 无法区分二者）。
- 加密文档的快照用同一格式、同一密钥写入；恢复时要求输入密码。
- 崩溃转储使用 `MiniDumpNormal`（不含堆），避免明文进入 dump。
- 明确告知用户的局限：编辑期间明文在进程内存、剪贴板中；无法防御本机恶意软件。
- 推荐扩展名 `.hne`，但识别只依据魔数。

### 6.10 主题与语法高亮（hn-sci）

- Lexilla 每个 lexer 的样式编号不同。定义一组**语义样式**（`Comment`、`Keyword`、`String`、`Number`、`Operator`、`Type`、`Function`、`Preprocessor`、`Tag`、`Attribute`、`Heading`、`Emphasis`、`Error`……），再为每个 lexer 写"lexer 样式号 → 语义样式"的映射表（数据文件，编译期嵌入）。
- 主题只为语义样式定义颜色。默认 Gruvbox Light：

  | 用途 | 颜色 |
  |---|---|
  | 背景 bg0 | `#fbf1c7` |
  | 前景 fg1 | `#3c3836` |
  | 当前行 / 选区 | `#ebdbb2` / `#d5c4a1` |
  | 注释 gray | `#928374` |
  | 关键字 red | `#9d0006` |
  | 字符串 green | `#79740e` |
  | 数字 / 常量 purple | `#8f3f71` |
  | 类型 yellow | `#b57614` |
  | 函数 / 标题 blue | `#076678` |
  | 预处理 / 属性 aqua | `#427b58` |
  | 运算符 / 标签 orange | `#af3a03` |

- 无级缩放：不用 `SCI_SETZOOM`（整数步进），而是按缩放系数对所有样式调用 `SCI_STYLESETSIZEFRACTIONAL`（单位为 1/100 磅），Ctrl+滚轮每格 ±5%。
- 首批语言：JSON、XML/HTML、Python、C/C++、SQL、YAML、Markdown、INI/TOML、Batch、PowerShell、JavaScript、CSS、Rust、Diff、Log（自定义简单 lexer 用于日志级别着色可放后期）。

### 6.11 Scintilla 已覆盖的编辑能力（只需开启/配置）

| 需求 | 实现 |
|---|---|
| 列选 Alt+拖拽、批量插入 | 默认支持；`SCI_SETMULTIPLESELECTION`、`SCI_SETADDITIONALSELECTIONTYPING`、`SCI_SETVIRTUALSPACEOPTIONS(SCVS_RECTANGULARSELECTION)` |
| Ctrl+点击多光标 | `SCI_SETMULTIPLESELECTION(true)` 后默认行为；`SCI_SETMULTIPASTE(SC_MULTIPASTE_EACH)` |
| 行号、折叠栏、书签栏 | margins + markers |
| 缩进线 | `SCI_SETINDENTATIONGUIDES(SC_IV_LOOKBOTH)` |
| 括号匹配高亮 | `SCI_BRACEMATCH` + `SCI_BRACEHIGHLIGHT`（在 `SCN_UPDATEUI` 中） |
| 空白/EOL 可视化 | `SCI_SETVIEWWS`、`SCI_SETVIEWEOL` |
| EOL 转换 | `SCI_CONVERTEOLS` + `SCI_SETEOLMODE` |
| 只读 | `SCI_SETREADONLY` |
| 移动行 | `SCI_MOVESELECTEDLINESUP/DOWN` |
| 修改历史标记 | `SCI_SETCHANGEHISTORY`（额外收益） |

需要自己写的编辑命令（在 `hn-text` 用纯函数实现，`hn-app` 负责取文本/写回）：
- **列表续写**：拦截 Enter（`SCN_CHARADDED` 不够，需在按键处理阶段），识别 `- ` `* ` `+ ` `1. ` `1) ` `- [ ] ` 及其缩进；空列表项再按 Enter 则删除前缀并结束列表。仅在 Markdown/纯文本中默认开启。
- **行操作**：排序（升/降、忽略大小写、按数字）、去重（保留首个，可选仅相邻）、Trim 行尾/行首、合并行、删除空行。对选区内的行操作，无选区时作用于全文。全部作为一次撤销。
- **书签批量操作**：复制/剪切/删除带书签行，删除不带书签行，反转书签。

### 6.12 侧边栏（hn-app）

- 代码：`app/sidebar.rs`（侧边栏与分隔条）、`app/views.rs`（分屏与 Ctrl+Tab）、`app/quick.rs`（Ctrl+P），都是 `app` 的子模块，可直接访问 `App` 的私有字段；控件句柄放在 `Cell` 中，避免控件发出的 `WM_NOTIFY` 重入时撞上 `RefCell` 借用。
- 上下两块：**打开的文件**（虚拟 ListView，`LVS_OWNERDATA`，文字在 `LVN_GETDISPINFO` 中按需生成：`*` 表示有修改，另有 `[加密]`、`[已删除]` 标记；`LVN_BEGINDRAG` + 捕获鼠标实现拖动排序）与**工作区**（TreeView，`TVN_ITEMEXPANDING` 时用 `ignore::WalkBuilder(max_depth=1)` 枚举，目录在前，每个目录最多 1 万项；窗口回到前台时同步已载入的目录，只增删变化的项，保留展开状态）。
- 图标：`SHGetFileInfoW(name, attr, SHGFI_USEFILEATTRIBUTES | SHGFI_SYSICONINDEX | SHGFI_SMALLICON)` 取系统图像列表索引，按扩展名缓存，不访问磁盘；列表与树共用系统图像列表（`LVS_SHAREIMAGELISTS`，不能销毁）。
- 分隔条不是子窗口：主窗口在布局时记下各分隔条矩形，自己处理 `WM_SETCURSOR` / `WM_LBUTTONDOWN` / `WM_MOUSEMOVE` / `WM_LBUTTONUP`（`SetCapture`）。侧边栏宽度用 DIP 存在设置中。
- **分屏**：两个 Scintilla 视图（第二个平时隐藏），共享文档（引用计数）。`sci` 始终指向焦点视图，`active` 是焦点视图中的文档，所有命令只作用于焦点视图；另一视图只显示。`SCN_FOCUSIN` 切换焦点视图；离开焦点视图前把位置、书签、折叠、只读等记到 `Doc`。激活一个已在另一视图显示的文档 = 把焦点移过去。关闭文档前，另一视图若显示它，改为显示焦点视图的文档。会话中 `view2` 记录非焦点视图的文档。
- **Ctrl+Tab**：在消息循环里预处理按键（`App::pretranslate`），弹出不抢焦点的列表（`WS_EX_NOACTIVATE`），按住 Ctrl 连按 Tab 选择，松开 Ctrl 时切换。**Ctrl+P**：非模态对话框，候选为已打开文档、最近文件与工作区文件索引（后台线程遍历，`PostMessage` 回传），输入变化时用 `hn_text::fuzzy` 排序。
- 侧边栏和快速打开里不带 Ctrl 的按键不走加速键表（否则 F2 会变成"下一个书签"而不是重命名）。

### 6.13 终端（hn-term + hn-app/term）

- 伪终端与 VT 状态机用 `alacritty_terminal` 0.26（Apache-2.0）：Windows 上它通过 ConPTY 启动 shell，exe 同目录（或 PATH）中有微软的 `conpty.dll` + `OpenConsole.exe` 时优先加载这个随附版本，否则用系统自带的 ConPTY——这就是"随附版 ConPTY"的接入方式，发布包可以选择是否附带这两个文件（MIT）。PTY 读取在它的事件循环线程中进行，状态在 `FairMutex<Term>` 里。
- 后台线程只通过回调通知：事件放进 `Mutex<Vec<TermEvent>>`，`pending` 标志为假时才 `PostMessage(WM_APP_TERM)` 给终端窗口自己，输出很多时也只有一条唤醒消息在队列中。
- 绘制：`Terminal::snapshot` 在锁内把可见区域复制成与平台无关的 `Snapshot`（颜色已按 Gruvbox Light 终端配色换算，处理 DIM/INVERSE/HIDDEN、选区），UI 线程释放锁后用 `hn-render` 画：同色背景合并成块；ASCII 按样式连续绘制（等宽字体前进宽度与格子一致），其他字符逐个画在自己的格子里（宽字符两格），避免回退字体的宽度把后面的字符挤歪；粗体、斜体用单独的 `Font`；下划线、删除线、块/竖线/下划线光标，失去焦点时为空心框。
- 键盘：方向键、Home/End、PgUp/PgDn、Insert/Delete、F1–F12 在 `WM_KEYDOWN` 中按 xterm 约定编码（修饰参数、DECCKM 应用光标模式）；其余字符走 `WM_CHAR`（Ctrl+字母已是控制字符；Backspace 发 DEL，Ctrl+Backspace 发 BS；代理对拼接）；`WM_SYSCHAR` → ESC 前缀（Alt+字符，不激活菜单）。终端获得焦点时，消息循环只让 Ctrl+` / Ctrl+Shift+` 走加速键表，其余按键都给终端。
- 复制粘贴：有选区时 Ctrl+C 复制，否则发 `\x03`；Ctrl+Shift+C/V、Ctrl+V、Shift+Insert；右键有选区时复制，否则粘贴；中键粘贴；括号粘贴模式下加标记并去掉文本中的结束标记。鼠标单击/双击/三击 = 字符/词/行选择。
- 滚轮：普通屏幕滚动回看（1 万行）；全屏程序（备用屏 + 备用滚动模式，如 vim、less）中换成方向键。Shift+PgUp/PgDn 翻页回看。暂不向程序报告鼠标事件。
- 输入法：`WM_IME_STARTCOMPOSITION` / `WM_IME_COMPOSITION` 时用 `ImmSetCompositionWindow(CFS_POINT)` 把组字窗口放到光标处，其余交给 `DefWindowProc`，上屏文字以 `WM_CHAR` 到达。
- 尺寸：窗口 `WM_SIZE` 时按格子尺寸算行列，`Terminal::resize` 同时调整 VT 网格与 PTY（尺寸没变时不发）。
- shell：设置 `terminal_shell`（powershell / pwsh / cmd 或完整路径），菜单可指定；工作目录 = 当前文件所在目录 → 第一个工作区目录 → 用户目录。shell 退出时关闭终端页；关闭终端时发 `Shutdown`，PTY 在事件循环线程中释放（`ClosePseudoConsole` 不在 UI 线程等待）。
- 多个终端：每个终端一个窗口、一个抽屉页签（"PowerShell 1"、"cmd 2"……）。

### 6.14 运行 Python（hn-run + app/runpy）

- 解释器探测顺序：设置 `python` 指定 → 脚本所在目录与工作区根目录下的 `.venv` / `venv` → `py -3` 启动器 → PATH 中的 `python.exe`（**排除 `WindowsApps` 下的商店占位程序**）。
- 启动参数：`python -u script.py`，环境变量 `PYTHONUTF8=1`、`PYTHONIOENCODING=utf-8`；工作目录 = 脚本所在目录；`CREATE_NO_WINDOW`。
- 运行前若文件有修改先保存；未命名文档写到 `%TEMP%\HurshNote\run\untitled-N.py` 运行。
- 子进程放入 Job Object（`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`），"停止"结束整棵进程树；`Running` 被丢弃（再次运行、程序退出）时 Job 句柄关闭，残留子进程也会结束。
- stdout / stderr 两根管道各一个读取线程，等待线程在两者读完后才发"退出"事件；事件经队列 + `PostMessage(WM_APP_RUN)` 送 UI（`pending` 合并唤醒，代号丢弃上一次运行的残留）。每个流一个增量解码器（拼接跨块的 UTF-8 与转义序列，只解释 SGR 前景色）；stderr 红色；结束时显示退出码与耗时。
- 输出面板是只读、无撤销的 Scintilla（由程序着色，超过 8 MB 时删掉开头）；双击 `File "x.py", line N`、`x.rs:12:5`、`x.cs(12,5)` 这类行跳转到源码（相对路径以脚本目录为基准）。
- 输出面板不支持 `input()`（stdin 为空）；Ctrl+F5 在新终端中运行（按 shell 种类拼好命令行写入终端）。

### 6.15 Markdown 预览（hn-md + hn-app）

- 编辑停止 150 ms 后在后台用 `pulldown-cmark` 全量重新解析（10 万字文档约数毫秒），生成**布局无关的块树**，回 UI 线程用 DirectWrite 排版并绘制。
- 支持：标题、段落、粗体/斜体/删除线、行内代码、代码块（等宽 + 背景，语法高亮放后期）、有序/无序/任务列表、引用、表格、分隔线、链接（点击用默认浏览器打开）、本地图片（WIC 解码，按宽度缩放，缓存）。
- 不支持：内嵌 HTML（按纯文本显示）、数学公式、远程图片（v1）。
- 滚动同步：块树记录源码行号，编辑器滚动时预览跟随，反之亦然。
- 预览中选中文本复制：M6 内实现基础版（按块选择）。

### 6.16 共享渲染设施（hn-render）

- 只在 UI 线程使用：`ID2D1Factory`、`IDWriteFactory` 按线程缓存；渲染目标是窗口的 `ID2D1HwndRenderTarget`，DPI 固定设为 96，坐标一律用物理像素，字号由调用方按 DPI 换算。
- `Surface`：渲染目标 + 一个纯色画刷（填充、线、文字、单行排版、裁剪）；`EndDraw` 返回 `D2DERR_RECREATE_TARGET` 时调用方丢弃重建。
- `Font`：文本格式与度量。用含中文的样本测行高与基线后设为**统一行距**，保证中英文混排时各行基线一致；`char_w` 为 "0" 的宽度（Hex 列、行号栏按它对齐）；制表位为 4 个字符宽。
- `TextLayout`：单行排版（不折行），提供命中测试（横坐标 ↔ UTF-16 位置）。
- 首次创建字体约 0.5 ms、渲染目标约 5 ms（CI 机器），不影响首屏预算。终端（M5）与 Markdown 预览（M6）沿用。

### 6.17 单实例与命令行

- 命名互斥量 `Local\HurshNote-{用户SID}`；已存在时找到主窗口，用 `WM_COPYDATA` 发送参数（JSON），然后退出。
- 参数：`<files...>`、`-n<行>`、`-c<列>`、`-ro`、`-multiInst`、`-nosession`、`--elevated-save`（内部）；测试用 `--selftest <输出>`、`--selftest-large <文件> <输出>`、`--selftest-session <阶段> <标记> <输出>`。

### 6.18 设置与便携模式

- 默认：`%APPDATA%\HurshNote\settings.toml`（serde + `#[serde(default)]`，缺字段取默认值；首次运行时迁移 M1/M2 的 `settings.ini`）。
- exe 同目录存在 `portable.flag` 时进入便携模式，所有数据写到 exe 同目录的 `data\`；环境变量 `HURSHNOTE_DATA_DIR` 可覆盖（测试用）。
- 设置变更即时生效，写文件同样走原子替换。

### 6.19 底部抽屉（app/drawer）

- 编辑区下方：顶部一条页签（`SysTabControl32` 按钮样式，右侧是状态文字：搜索进度、运行状态），下面是当前页面窗口；高度可拖动（分隔条同侧边栏，DIP 存在设置 `drawer_height`）。
- 页面：**搜索结果**（虚拟 ListView，大文件搜索的匹配与目录搜索的"文件行 + 匹配行"共用；双击/回车跳转，Esc 隐藏）、**输出**（运行 Python、目录替换的问题列表）、**终端**（每个终端一页）。
- 快捷键：F7 搜索结果；Ctrl+` 显示/隐藏抽屉（没有终端时新建一个）；Ctrl+Shift+` 新建终端；Ctrl+Shift+F 在文件中查找；F5 / Ctrl+F5 / Shift+F5 运行、在终端中运行、停止。菜单"运行"汇总这些命令。

## 7. 性能预算（每个里程碑都要回归）

| 指标 | 目标 |
|---|---|
| 冷启动到可输入（空会话） | < 150 ms |
| 冷启动（20 个文件的会话） | < 300 ms（懒加载） |
| 空载内存 | < 30 MB |
| exe 体积 | < 10 MB |
| 打开 100 MB 文本 | 界面不冻结；< 2 s 可编辑 |
| 打开 5 GB 日志（大文件视图） | 首屏 < 300 ms；滚动 60 fps |
| 目录搜索 | 与同条件 ripgrep 的耗时差距 < 30% |
| 快照 | 1 MB 文档快照写入 < 20 ms，不占用 UI 线程 |

## 8. 测试策略

- **单元测试**：`hn-text`、`hn-search`、`hn-crypto`、`hn-core`、`hn-large`（以后的 `hn-md`）在 Linux 和 Windows CI 上都跑。
- **大文件性能测试**：CI 生成 5 GB 日志（`scripts/gen_big_log.py`，生成后 fsync），用 `--selftest-large` 测首屏、跳到中间/末尾、连续翻页、建索引、跳转到中间行的耗时。
- **属性测试**（`proptest`）：编码往返、EOL 转换、行操作的不变式（排序后行集合不变、去重幂等）。
- **模糊测试**（`cargo-fuzz`）：加密文件头解析、编码嗅探、Markdown 解析到布局、行索引。
- **集成自测**：exe 内置隐藏的 `--selftest <脚本>` 模式，在真实窗口中执行脚本化操作（打开、编辑、保存、切换编码）并以退出码报告结果，在 Windows CI 上运行。
- **强杀测试**：脚本启动程序 → 通过 `WM_COPYDATA` 注入编辑 → `TerminateProcess` → 重启 → 校验内容，循环数百次。
- **语料**：`tests/corpus/` 收集 GBK、GB18030 四字节字符、UTF-16 LE/BE 有/无 BOM、混合换行、100 MB 单行 JSON、二进制、损坏的加密文件等；大文件由脚本生成，不入库。
