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
| Win32 绑定 | `windows-sys`（普通 Win32 API，零开销、签名稳定）；需要 COM（DirectWrite / Direct2D / WIC / Shell）时再引入 `windows` crate | 不引入任何 GUI 框架 |
| 编辑内核 | Scintilla 5.x + Lexilla 5.x，源码放在 `third_party/`，用 `cc` crate 以 C++17 编译并**静态链接** | 静态时调用 `Scintilla_RegisterClasses`；lexer 用 `CreateLexer` + `SCI_SETILEXER` |
| 文本渲染 | Scintilla 用 `SC_TECHNOLOGY_DIRECTWRITE`；自绘视图用 Direct2D + DirectWrite | 高 DPI、彩色 emoji、字体回退 |
| 编码 | `encoding_rs` + `chardetng` | GB18030 往返无损 |
| 正则 | `pcre2`（JIT），编辑器内查找与目录搜索共用 | 支持环视、反向引用，行为与 N++ 用户习惯接近 |
| 目录搜索 | `ignore`（并行遍历 + gitignore）+ `grep-searcher` + `grep-pcre2` | ripgrep 的组件 |
| 大文件 | `memmap2` + `memchr` | 页访问异常保护见 §6.4 |
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
    hn-large     分块读取/mmap、稀疏行索引、大文件搜索、Hex 数据源、Tail       ← 核心逻辑跨平台可测
    hn-search    查找替换引擎、目录搜索                                         ← 跨平台可测
    hn-crypto    加密文件格式读写                                               ← 跨平台可测
    hn-md        Markdown → 布局树（与平台无关）；绘制在 hn-app
    hn-term      ConPTY 封装 + VT 状态 + 输入映射
    hn-run       Python 解释器探测、进程启动、Job Object、输出流
    hn-render    共享的 D2D/DWrite 设施：字体、颜色、文本布局缓存（Hex、大文件、终端、MD 共用）
  tests/corpus/  测试语料（编码、换行、超长行、加密样本……）
  docs/
```

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
 ├─ size > large_threshold (256MB) ───────────────→ 大文件视图（只读）
 ├─ 前 8KB 含 NUL 且不像 UTF-16 ──────────────────→ 提示：用 Hex 视图打开 / 强制按文本打开
 ├─ 魔数 = HURSHENC ──────────────────────────────→ 输入密码 → 解密 → 编辑器
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

### 6.4 大文件视图（hn-large）

- **只读**。打开时立即显示第一屏（只需读前几 KB），后台线程用 `memchr` 扫描换行，每 1 MB 记一个检查点（稀疏行索引），进度显示在状态栏；索引完成前滚动条按字节比例估算。
- 数据访问：静态文件用 `memmap2`，以 `FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE` 打开。**所有页访问经过一个小的 C 函数包裹 SEH（`__try/__except`）**，捕获 `EXCEPTION_IN_PAGE_ERROR` 后转为错误返回，不让进程崩溃。
- 编码：只支持 UTF-8 / ANSI / GB18030 / UTF-16 的逐块解码，块边界对齐到字符边界。
- 搜索：后台按块流式匹配（`grep-searcher` 支持任意 reader），结果进底部列表，点击跳转。
- 渲染：`hn-render` 自绘，只排版可见行；长行横向截断显示（> 10 000 字符的行显示前段 + "…"）。
- **Tail 模式**：不使用 mmap。监听文件变化后用 `ReadFile` 从上次偏移读增量；检测到文件变小（被轮转/截断）时从头重读并提示。

### 6.5 Hex 视图

- 数据源抽象：
  ```rust
  trait ByteSource { fn len(&self) -> u64; fn read_at(&self, off: u64, buf: &mut [u8]) -> Result<usize>; }
  ```
  实现：`MmapSource`（磁盘文件）、`BufferSource`（编辑中的文档按当前编码编码后的字节快照）。
- 固定 16 字节一行、虚拟滚动，只读取可见范围。列：偏移 | 16 字节 Hex | ASCII/当前编码解码。
- 光标在文本视图与 Hex 视图间切换时换算字节偏移，切换后定位不丢失。
- v1 只读；覆盖式 Hex 编辑放到 v1 之后。

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
- Tail 模式（M4）需要更短的间隔，届时为单个文件单独计时。

### 6.8 查找替换（hn-search）

- 编辑器内：`SCI_GETCHARACTERPOINTER` 拿连续缓冲区 → PCRE2 匹配 → `SCI_SETSEL` / `SCI_REPLACETARGET`。全部替换在一个 `SCI_BEGINUNDOACTION` / `SCI_ENDUNDOACTION` 里完成，并且从后往前替换以避免偏移失效。
- 三种模式：普通 / 转义（`\n \r \t \0 \xNN \uNNNN`）/ 正则；选项：大小写、全词、仅选区、循环、方向。
- 目录搜索：后台 `ignore::WalkParallel` + `grep-searcher`；跳过二进制文件、加密文件（魔数检测）、超过阈值的文件（可选）；结果按文件分组，每 50 ms 一批送 UI；可取消。
- 在目录中替换：先预览，再逐文件走与"保存"相同的安全写入流程；已打开且有修改的文件改为在编辑器缓冲区内替换。

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

- 上下两块：**打开的文件**（ListView，显示 Dirty 圆点、只读锁、加密标记，可拖动排序，可按目录分组）与**工作区**（TreeView，懒加载子目录，可选遵守 `.gitignore`，由文件监控驱动刷新）。
- 图标：`SHGetFileInfoW(path, FILE_ATTRIBUTE_NORMAL, SHGFI_USEFILEATTRIBUTES | SHGFI_SYSICONINDEX | SHGFI_SMALLICON)` 取系统图像列表索引，按扩展名缓存；`.exe/.lnk/.ico` 这类需要读文件的图标在后台 STA 线程取，取到前用通用图标。
- 工作区可保存为 `.hnworkspace`（TOML：根目录列表、排除规则）。
- **Ctrl+Tab**：弹出 MRU 列表，按住 Ctrl 连按 Tab 选择，松开切换。**Ctrl+P**：模糊匹配已打开文件 + 工作区文件（后台建文件名索引）。

### 6.13 终端（hn-term）

- `CreatePseudoConsole` + 两根管道；一个线程读输出喂给 `alacritty_terminal` 的 VT 状态机，UI 线程按帧绘制网格。
- 默认 shell：PowerShell 5.1（Win10 自带，带 PSReadLine，补全由它提供）；可选 `cmd.exe`、`pwsh.exe`（若已安装）。工作目录 = 当前文件所在目录或工作区根目录。
- 宽字符按 East Asian Width 占两格；字体回退由 DirectWrite 完成。
- 自绘控件自行处理 IME：`WM_IME_STARTCOMPOSITION` 时用 `ImmSetCompositionWindow` 把候选窗定位到光标处，`WM_IME_COMPOSITION` 取结果字符串写入 PTY。
- 复制/粘贴：有选区时 Ctrl+C 复制，否则发送 `\x03`；Ctrl+V 粘贴（多行粘贴前提示）。
- 关闭：在独立线程继续排空输出管道的同时调用 `ClosePseudoConsole`，避免死锁；用 Job Object 确保子进程树退出。
- 1809 自带 ConPTY 有已知问题；评估随程序附带微软的 ConPTY 包（`conpty.dll` + `OpenConsole.exe`，MIT）在 1809 上是否可用，可用则优先加载随附版本。

### 6.14 运行 Python（hn-run）

- 解释器探测顺序：设置中指定 → 工作区内 `.venv` / `venv` → `py -3` 启动器 → PATH 中的 `python.exe`（**排除 `WindowsApps` 下的商店占位程序**）。
- 启动参数：`python -u script.py`，环境变量 `PYTHONUTF8=1`、`PYTHONIOENCODING=utf-8`；工作目录 = 脚本所在目录。
- 运行前若文件有修改：自动保存（可设置），未命名文件写到临时目录运行。
- 子进程放入 Job Object（`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`），"停止"杀掉整棵进程树。
- stdout / stderr 两根管道，两个读取线程，合批送 UI；stderr 用红色；支持基础 ANSI SGR 颜色。
- 输出中的 `File "x.py", line N` 可点击跳转。
- 输出面板不支持 `input()`；提供"在终端中运行"命令（向终端发送 `python "x.py"`）。

### 6.15 Markdown 预览（hn-md + hn-app）

- 编辑停止 150 ms 后在后台用 `pulldown-cmark` 全量重新解析（10 万字文档约数毫秒），生成**布局无关的块树**，回 UI 线程用 DirectWrite 排版并绘制。
- 支持：标题、段落、粗体/斜体/删除线、行内代码、代码块（等宽 + 背景，语法高亮放后期）、有序/无序/任务列表、引用、表格、分隔线、链接（点击用默认浏览器打开）、本地图片（WIC 解码，按宽度缩放，缓存）。
- 不支持：内嵌 HTML（按纯文本显示）、数学公式、远程图片（v1）。
- 滚动同步：块树记录源码行号，编辑器滚动时预览跟随，反之亦然。
- 预览中选中文本复制：M6 内实现基础版（按块选择）。

### 6.16 单实例与命令行

- 命名互斥量 `Local\HurshNote-{用户SID}`；已存在时找到主窗口，用 `WM_COPYDATA` 发送参数（JSON），然后退出。
- 参数：`<files...>`、`-n<行>`、`-c<列>`、`-ro`、`-multiInst`、`-nosession`、`--elevated-save`（内部）。

### 6.17 设置与便携模式

- 默认：`%APPDATA%\HurshNote\settings.toml`。
- exe 同目录存在 `portable.flag` 或 `settings.toml` 时进入便携模式，所有数据写到 exe 同目录。
- 设置变更即时生效，写文件同样走原子替换。

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

- **单元测试**：`hn-text`、`hn-search`、`hn-crypto`、`hn-large`、`hn-md` 在 Linux 和 Windows CI 上都跑。
- **属性测试**（`proptest`）：编码往返、EOL 转换、行操作的不变式（排序后行集合不变、去重幂等）。
- **模糊测试**（`cargo-fuzz`）：加密文件头解析、编码嗅探、Markdown 解析到布局、行索引。
- **集成自测**：exe 内置隐藏的 `--selftest <脚本>` 模式，在真实窗口中执行脚本化操作（打开、编辑、保存、切换编码）并以退出码报告结果，在 Windows CI 上运行。
- **强杀测试**：脚本启动程序 → 通过 `WM_COPYDATA` 注入编辑 → `TerminateProcess` → 重启 → 校验内容，循环数百次。
- **语料**：`tests/corpus/` 收集 GBK、GB18030 四字节字符、UTF-16 LE/BE 有/无 BOM、混合换行、100 MB 单行 JSON、二进制、损坏的加密文件等；大文件由脚本生成，不入库。
