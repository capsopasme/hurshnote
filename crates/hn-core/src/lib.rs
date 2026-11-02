//! HurshNote 核心数据格式（与平台无关，可在 Linux 上测试）。
//!
//! - [`stamp`]：磁盘文件戳（修改时间 + 大小 + 内容哈希），用于判断文件是否被外部修改
//! - [`atomic`]：原子写入（临时文件 → 刷盘 → 改名）
//! - [`payload`]：文档内容 + 元数据的二进制布局（加密文档与快照共用）
//! - [`snapshot`]：快照文件格式与快照目录管理
//! - [`session`]：会话清单（JSON）
//! - [`macros`]：宏的步骤序列与持久化

pub mod atomic;
pub mod macros;
pub mod payload;
pub mod session;
pub mod snapshot;
pub mod stamp;
