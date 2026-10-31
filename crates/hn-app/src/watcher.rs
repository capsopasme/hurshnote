//! 外部文件变更监控。
//!
//! 采用轮询（每秒检查一次修改时间与大小）而不是 ReadDirectoryChangesW：
//! 打开的文件通常只有几十个，轮询开销可以忽略；同时天然支持网络盘、"写临时文件再改名"式保存，
//! 不需要处理通知缓冲区溢出与句柄占用目录的问题。
//!
//! 后台线程只负责发现"元数据变了"，是否真的变化（内容哈希）由 UI 线程与文档的磁盘戳比较后决定。

use std::collections::HashMap;
use std::os::windows::fs::MetadataExt;
use std::path::PathBuf;
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::time::Duration;

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW;

const INTERVAL: Duration = Duration::from_millis(1000);

enum Cmd {
    Watch(u32, PathBuf),
    Unwatch(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Changed(u32),
    Deleted(u32),
}

pub struct Watcher {
    tx: Sender<Cmd>,
}

type Meta = Option<(u64, u64)>;

fn stat(p: &PathBuf) -> Meta {
    std::fs::metadata(p)
        .ok()
        .map(|m| (m.last_write_time(), m.len()))
}

impl Watcher {
    /// 启动监控线程。事件通过 `events` 发送，并向 `hwnd` 投递 `msg` 唤醒 UI。
    pub fn start(hwnd: HWND, msg: u32, events: Sender<Event>) -> Watcher {
        let (tx, rx) = channel::<Cmd>();
        let hwnd = hwnd as isize;
        std::thread::Builder::new()
            .name("file-watcher".into())
            .spawn(move || {
                let mut files: HashMap<u32, (PathBuf, Meta)> = HashMap::new();
                loop {
                    match rx.recv_timeout(INTERVAL) {
                        Ok(Cmd::Watch(id, path)) => {
                            let m = stat(&path);
                            files.insert(id, (path, m));
                            continue;
                        }
                        Ok(Cmd::Unwatch(id)) => {
                            files.remove(&id);
                            continue;
                        }
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                    let mut fired = false;
                    for (id, (path, last)) in files.iter_mut() {
                        let cur = stat(path);
                        if cur != *last {
                            *last = cur;
                            let ev = if cur.is_none() {
                                Event::Deleted(*id)
                            } else {
                                Event::Changed(*id)
                            };
                            if events.send(ev).is_err() {
                                return;
                            }
                            fired = true;
                        }
                    }
                    if fired {
                        unsafe { PostMessageW(hwnd as HWND, msg, 0, 0) };
                    }
                }
            })
            .expect("无法创建文件监控线程");
        Watcher { tx }
    }

    /// 开始（或重新）监控。以当前磁盘状态为基准，之后的变化才会通知。
    pub fn watch(&self, id: u32, path: PathBuf) {
        let _ = self.tx.send(Cmd::Watch(id, path));
    }

    pub fn unwatch(&self, id: u32) {
        let _ = self.tx.send(Cmd::Unwatch(id));
    }
}
