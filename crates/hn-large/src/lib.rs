//! 大文件核心逻辑（跨平台，只依赖 std，可在 Linux 上测试）。
//!
//! - [`source`]：字节源（磁盘文件 / 内存缓冲区）与 UI 线程使用的块缓存读取器
//! - [`charset`]：按编码族判断字符边界与换行符（UTF-8 / UTF-16 / GB18030 / 双字节 / 单字节）
//! - [`index`]：稀疏行索引（每 64 KiB 记一次累计换行数），后台线程分批构建
//! - [`rows`]：显示行导航（超长行按固定字节宽度折成多个显示行）
//! - [`decode`]：显示行解码为 UTF-16，并记录每个码元对应的源字节偏移
//! - [`search`]：按行对齐的分块遍历（正向、反向），供流式搜索使用
//! - [`hex`]：Hex 视图的行格式与列命中测试
//! - [`tail`]：文件状态快照与变化分类（增长 / 截断 / 被替换 / 被改写）
//!
//! 不使用内存映射：所有读取都是"打开 → 定位读 → 关闭"，不长期持有文件句柄，
//! 因此写日志的程序可以随时轮转、截断、删除文件；文件被外部截断也不会触发页访问异常。

pub mod charset;
pub mod decode;
pub mod hex;
pub mod index;
pub mod rows;
pub mod search;
pub mod source;
pub mod tail;

pub use charset::Charset;
pub use index::{LineIndex, BLOCK};
pub use source::{ByteSource, FileSource, MemSource, Reader};
