//! 运行 Python（只依赖 std，可在 Linux 上测试）。
//!
//! - [`interp`]：解释器探测（设置 → 工作区虚拟环境 → `py` 启动器 → PATH，排除商店占位程序）
//! - [`proc`]：启动子进程、两个线程分别读 stdout/stderr、等待退出；Windows 上放进 Job Object，
//!   "停止"会结束整棵进程树，程序退出时子进程也随之结束
//! - [`parse`]：Traceback 中的 `File "x.py", line N` 定位
//! - [`ansi`]：ANSI SGR 颜色序列解析（其他控制序列丢弃），支持跨块的不完整序列与 UTF-8 字符

pub mod ansi;
pub mod interp;
pub mod parse;
pub mod proc;
