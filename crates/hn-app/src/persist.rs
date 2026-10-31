//! 快照与会话恢复（`app` 的子模块，可访问 App 的私有字段）。
//!
//! - 内容变化后防抖 1.5 s 写快照；连续编辑时最长 10 s 写一次；切换文档、退出时立即写
//! - 快照与会话清单由单独的写入线程按顺序落盘：先快照、后清单；删除旧快照排在清单之后
//! - 恢复时只载入当前文档，其余文档在被激活时才载入（懒加载）

use std::sync::mpsc::{channel, Sender};
use std::time::Duration;

use hn_core::atomic::write_atomic;
use hn_core::payload::Payload;
use hn_core::session::{Session, SessionDoc};
use hn_core::snapshot::{self, Snapshot, Store};
use hn_crypto::Key;
use hn_text::encoding::TextEncoding;
use windows_sys::Win32::System::SystemInformation::GetTickCount64;
use windows_sys::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};

use super::*;
use crate::docs::{DocInit, ViewState};

pub(super) const TIMER_SNAPSHOT: usize = 2;
pub(super) const TIMER_SESSION: usize = 3;
pub(super) const TIMER_SESSIONTEST: usize = 4;
const SNAPSHOT_DELAY_MS: u32 = 1500;
const SNAPSHOT_MAX_LATENCY_MS: u64 = 10_000;
const SESSION_DELAY_MS: u32 = 2000;

pub(super) enum SnapJob {
    Write {
        name: String,
        payload: Payload,
        key: Option<Key>,
    },
    Remove(String),
    Session(String),
    Flush(Sender<()>),
}

pub(super) fn session_path() -> PathBuf {
    config::data_dir().join("session.json")
}

/// 启动快照写入线程。
pub(super) fn start_writer(store: Store) -> Sender<SnapJob> {
    let (tx, rx) = channel::<SnapJob>();
    std::thread::Builder::new()
        .name("snapshot-writer".into())
        .spawn(move || {
            let session = session_path();
            for job in rx {
                match job {
                    SnapJob::Write { name, payload, key } => {
                        if let Ok(bytes) = snapshot::encode(&payload, key.as_ref()) {
                            let _ = store.write(&name, &bytes);
                        }
                    }
                    SnapJob::Remove(name) => store.remove(&name),
                    SnapJob::Session(json) => {
                        let _ = write_atomic(&session, json.as_bytes());
                    }
                    SnapJob::Flush(done) => {
                        let _ = done.send(());
                    }
                }
            }
        })
        .expect("无法创建快照线程");
    tx
}

/// 当前文档的书签行与折叠起来的行。
pub(super) fn capture_marks(sci: &Sci) -> (Vec<usize>, Vec<usize>) {
    let mut bookmarks = Vec::new();
    let mask = 1isize << theme::MARK_BOOKMARK;
    let mut line = 0usize;
    loop {
        let n = sci.send(SCI_MARKERNEXT, line, mask);
        if n < 0 {
            break;
        }
        bookmarks.push(n as usize);
        line = n as usize + 1;
    }
    let mut folds = Vec::new();
    let mut line = 0usize;
    loop {
        let n = sci.send(SCI_CONTRACTEDFOLDNEXT, line, 0);
        if n < 0 {
            break;
        }
        folds.push(n as usize);
        line = n as usize + 1;
    }
    (bookmarks, folds)
}

impl App {
    /// 快照功能是否启用（自测模式下关闭，避免污染真实会话）。
    pub(super) fn snapshots_enabled(&self) -> bool {
        self.selftest_out.is_none() && self.settings.borrow().snapshots
    }

    /// 当前文档内容变化：标记并安排快照。
    pub(super) fn mark_changed(&self) {
        let id = self.active.get();
        if let Ok(mut docs) = self.docs.try_borrow_mut() {
            if let Some(d) = docs.iter_mut().find(|d| d.id == id) {
                d.snap_stale = true;
            }
        }
        self.arm_snapshot_timer();
    }

    pub(super) fn arm_snapshot_timer(&self) {
        if !self.snapshots_enabled() {
            return;
        }
        let now = unsafe { GetTickCount64() };
        if !self.snap_armed.get() {
            self.snap_armed.set(true);
            self.snap_first.set(now);
        }
        // 超过最长延迟就不再推迟，让已排的定时器到点触发
        if now - self.snap_first.get() < SNAPSHOT_MAX_LATENCY_MS {
            unsafe { SetTimer(self.hwnd(), TIMER_SNAPSHOT, SNAPSHOT_DELAY_MS, None) };
        }
    }

    pub(super) fn arm_session_timer(&self) {
        if self.snapshots_enabled() {
            unsafe { SetTimer(self.hwnd(), TIMER_SESSION, SESSION_DELAY_MS, None) };
        }
    }

    pub(super) fn on_persist_timer(&self, id: usize) {
        unsafe { KillTimer(self.hwnd(), id) };
        if id == TIMER_SNAPSHOT {
            self.snap_armed.set(false);
            self.snapshot_active();
        }
        self.write_session();
    }

    /// 为当前文档写快照（或在文档已无未保存内容时删除快照）。
    pub(super) fn snapshot_active(&self) {
        if !self.snapshots_enabled() {
            return;
        }
        let Some(sci) = self.sci() else { return };
        let id = self.active.get();
        let Some((untitled, meta_dirty, stale, snap, enc, lang, key, loaded)) =
            self.with_doc(id, |d| {
                (
                    d.path.is_none(),
                    d.meta_dirty,
                    d.snap_stale,
                    d.snapshot.clone(),
                    d.enc,
                    d.lang,
                    d.key.clone(),
                    d.ptr != 0,
                )
            })
        else {
            return;
        };
        if !loaded {
            return;
        }
        let needs = sci.is_modified() || meta_dirty || (untitled && !sci.is_empty());
        if needs {
            if !stale && snap.is_some() {
                return;
            }
            let name = snap.unwrap_or_else(|| self.store.new_name());
            let payload = Payload::new(sci.bytes())
                .with("encoding", &enc.encoding.id())
                .with("bom", if enc.bom { "1" } else { "0" })
                .with("eol", docs::eol_to_str(editops::eol_of(&sci)))
                .with("lang", LANGS[lang].id);
            let _ = self.snap_tx.send(SnapJob::Write {
                name: name.clone(),
                payload,
                key,
            });
            self.with_doc_mut(id, |d| {
                d.snapshot = Some(name);
                d.snap_stale = false;
            });
            self.arm_session_timer();
        } else if let Some(old) = snap {
            self.with_doc_mut(id, |d| {
                d.snapshot = None;
                d.snap_stale = false;
            });
            self.to_remove.borrow_mut().push(old);
            self.arm_session_timer();
        }
    }

    fn build_session(&self) -> Session {
        let sci = self.sci();
        let active = self.active.get();
        // 当前文档的实时状态
        let live = sci.map(|s| {
            let (bm, folds) = capture_marks(&s);
            (
                Self::capture_view(&s),
                bm,
                folds,
                editops::eol_of(&s),
                s.is_read_only(),
            )
        });
        let docs = self.docs.borrow();
        let mut out = Session::default();
        for (i, d) in docs.iter().enumerate() {
            if d.id == active {
                out.active = i;
            }
            if let Some(sd) = &d.pending {
                out.docs.push(sd.clone());
                continue;
            }
            let (view, bookmarks, folds, eol, ro) = match (&live, d.id == active) {
                (Some((v, b, f, e, r)), true) => (*v, b.clone(), f.clone(), *e, *r),
                _ => (
                    d.view,
                    d.bookmarks.clone(),
                    d.folds.clone(),
                    d.eol,
                    d.read_only,
                ),
            };
            out.docs.push(SessionDoc {
                path: d.path.clone(),
                untitled: d.untitled,
                snapshot: d.snapshot.clone(),
                encoding: d.enc.encoding.id(),
                bom: d.enc.bom,
                eol: docs::eol_to_str(eol).into(),
                lang: LANGS[d.lang].id.into(),
                read_only: ro,
                encrypted: d.key.is_some(),
                meta_dirty: d.meta_dirty,
                anchor: view.anchor,
                caret: view.caret,
                first_line: view.first_line,
                bookmarks,
                folds,
                disk: d.disk,
            });
        }
        out
    }

    /// 写会话清单，然后删除不再引用的快照。
    pub(super) fn write_session(&self) {
        if !self.snapshots_enabled() {
            return;
        }
        let json = self.build_session().to_json();
        let _ = self.snap_tx.send(SnapJob::Session(json));
        for name in self.to_remove.borrow_mut().drain(..) {
            let _ = self.snap_tx.send(SnapJob::Remove(name));
        }
    }

    /// 立即写入快照与会话，并等待写入线程完成（退出、注销时调用）。
    pub(super) fn flush_persist(&self) {
        if !self.snapshots_enabled() {
            return;
        }
        self.snapshot_active();
        self.write_session();
        let (tx, rx) = channel();
        if self.snap_tx.send(SnapJob::Flush(tx)).is_ok() {
            let _ = rx.recv_timeout(Duration::from_secs(15));
        }
    }

    /// 启动时恢复上次的会话。返回是否恢复了文档。
    pub(super) fn restore_session(&self) -> bool {
        let Ok(text) = std::fs::read_to_string(session_path()) else {
            return false;
        };
        let Ok(session) = Session::from_json(&text) else {
            return false;
        };
        self.store.cleanup(&session.snapshot_names());
        if session.docs.is_empty() {
            return false;
        }
        let initial = self.pristine_untitled();
        let mut ids = Vec::new();
        let mut max_untitled = 0;
        for sd in session.docs {
            let id = self.alloc_id();
            let mut d = Doc::new(id, 0);
            d.path = sd.path.clone();
            d.untitled = sd.untitled;
            max_untitled = max_untitled.max(sd.untitled);
            d.lang = lang::by_id(&sd.lang).map(lang::index_of).unwrap_or(0);
            d.enc = TextEncoding::from_id(&sd.encoding)
                .map(|e| EncodingInfo::new(e, sd.bom))
                .unwrap_or(EncodingInfo::UTF8);
            d.snapshot = sd.snapshot.clone();
            d.modified = sd.snapshot.is_some() || sd.meta_dirty;
            d.disk = sd.disk;
            d.pending = Some(sd);
            self.docs.borrow_mut().push(d);
            ids.push(id);
        }
        self.untitled_seq
            .set(self.untitled_seq.get().max(max_untitled));
        let target = ids[session.active.min(ids.len() - 1)];
        self.activate(target);
        if self.active.get() != target {
            // 当前文档未能载入（例如取消了密码），尝试其他文档
            for &id in &ids {
                self.activate(id);
                if self.active.get() == id {
                    break;
                }
            }
        }
        if let Some(p) = initial {
            if self.active.get() != p {
                self.close_doc_unchecked(p);
            }
        }
        true
    }

    /// 由 UTF-8 文本创建 Scintilla 文档（引用计数 1）。
    pub(super) fn document_from_text(&self, text: &[u8]) -> Option<isize> {
        let sci = self.sci()?;
        let mut loader = sci.create_loader(text.len())?;
        if !loader.add(text) {
            return None;
        }
        Some(loader.into_document())
    }

    /// 懒加载：载入会话中尚未载入的文档。返回 false 表示放弃（例如取消输入密码）。
    pub(super) fn load_pending(&self, id: u32) -> bool {
        let Some(sci) = self.sci() else { return false };
        let Some(sd) = self.with_doc_mut(id, |d| d.pending.take()).flatten() else {
            return false;
        };
        let name = self.with_doc(id, |d| d.title()).unwrap_or_default();
        let hwnd = self.hwnd();

        let mut text: Option<Vec<u8>> = None;
        let mut ptr: Option<isize> = None;
        let mut key: Option<Key> = None;
        let mut from_snapshot = false;
        let mut disk = sd.disk;
        let mut enc = TextEncoding::from_id(&sd.encoding)
            .map(|e| EncodingInfo::new(e, sd.bom))
            .unwrap_or(EncodingInfo::UTF8);
        let mut eol = docs::eol_from_str(&sd.eol);
        let (mut lossy, mut binary, mut missing) = (false, false, false);

        // 1. 快照
        if let Some(snap) = &sd.snapshot {
            match self.store.read(snap) {
                Ok(Snapshot::Plain(p)) => text = Some(p.text),
                Ok(Snapshot::Encrypted(sealed)) => {
                    let prompt = format!("恢复未保存的加密文档“{name}”，请输入密码：");
                    match self.ask_password_loop(&prompt, |pw| {
                        snapshot::open_encrypted_with_password(&sealed, pw)
                            .map_err(|e| e.to_string())
                    }) {
                        Some((k, p)) => {
                            text = Some(p.text);
                            key = Some(k);
                        }
                        None => {
                            self.with_doc_mut(id, |d| d.pending = Some(sd));
                            return false;
                        }
                    }
                }
                Err(e) => dialogs::error_box(
                    hwnd,
                    &format!("无法读取“{name}”的快照：{e}\n将改为从磁盘载入。"),
                ),
            }
            if text.is_some() {
                from_snapshot = true;
                if let (Some(path), Some(st)) = (&sd.path, sd.disk) {
                    if let Some(cur) = docs::disk_stamp(path) {
                        if cur.hash != st.hash || cur.size != st.size {
                            let q = format!(
                                "“{name}”在上次关闭后被其他程序修改过。\n\n是：保留上次未保存的内容\n否：载入磁盘上的新版本"
                            );
                            if dialogs::ask_yes_no_cancel(hwnd, &q) == Answer::Yes {
                                disk = Some(cur);
                            } else {
                                text = None;
                                from_snapshot = false;
                                key = None;
                            }
                        }
                    }
                }
            }
        }

        // 2. 磁盘
        if text.is_none() {
            match &sd.path {
                Some(path) if !path.exists() => {
                    missing = true;
                    text = Some(Vec::new());
                }
                Some(path) if docs::is_encrypted_file(path) => {
                    match self.open_encrypted_file(path) {
                        Some((k, p, st)) => {
                            eol = p.get("eol").and_then(docs::eol_from_str).or(eol);
                            text = Some(p.text);
                            key = Some(k);
                            disk = Some(st);
                        }
                        None => {
                            self.with_doc_mut(id, |d| d.pending = Some(sd));
                            return false;
                        }
                    }
                }
                Some(path) => match (docs::check_size(path), sci.create_loader(0)) {
                    (Ok(_), Some(loader)) => {
                        let job = LoadJob {
                            path: path.clone(),
                            forced: TextEncoding::from_id(&sd.encoding),
                            reuse: None,
                            goto: None,
                            read_only: sd.read_only,
                        };
                        match docs::load(job, loader, self.hints).result {
                            Ok(t) => {
                                enc = t.info;
                                lossy = t.lossy;
                                binary = t.binary;
                                eol = Some(t.eol);
                                disk = Some(t.disk);
                                ptr = Some(t.loader.into_document());
                            }
                            Err(e) => {
                                dialogs::error_box(
                                    hwnd,
                                    &format!("无法打开 {}\n\n{e}", path.display()),
                                );
                                text = Some(Vec::new());
                            }
                        }
                    }
                    (Err(e), _) => {
                        dialogs::error_box(hwnd, &format!("无法打开 {}\n\n{e}", path.display()));
                        text = Some(Vec::new());
                    }
                    _ => text = Some(Vec::new()),
                },
                None => text = Some(Vec::new()),
            }
        }

        let long_lines = text
            .as_ref()
            .is_some_and(|t| hn_text::encoding::max_line_len(t) > docs::LONG_LINE_LIMIT);
        let ptr = match ptr {
            Some(p) => p,
            None => match self.document_from_text(&text.unwrap_or_default()) {
                Some(p) => p,
                None => {
                    self.with_doc_mut(id, |d| d.pending = Some(sd));
                    return false;
                }
            },
        };
        if sd.snapshot.is_some() && !from_snapshot {
            self.to_remove.borrow_mut().extend(sd.snapshot.clone());
        }
        let path = sd.path.clone();
        self.with_doc_mut(id, |d| {
            d.ptr = ptr;
            d.enc = enc;
            d.lossy = lossy;
            d.binary = binary;
            d.long_lines = d.long_lines || long_lines;
            d.key = key;
            d.disk = disk;
            d.missing = missing;
            d.meta_dirty = from_snapshot || sd.meta_dirty;
            d.modified = false;
            d.snapshot = if from_snapshot {
                sd.snapshot.clone()
            } else {
                None
            };
            d.snap_stale = false;
            d.init = Some(DocInit {
                eol,
                read_only: sd.read_only,
                bookmarks: sd.bookmarks.clone(),
                folds: sd.folds.clone(),
                view: Some(ViewState {
                    anchor: sd.anchor,
                    caret: sd.caret,
                    first_line: sd.first_line,
                    x_offset: 0,
                }),
            });
        });
        if let Some(p) = path {
            if !missing {
                self.watch(id, &p);
            }
        }
        true
    }

    /// 文档刚挂到视图上：设置换行符、语言、只读、书签、折叠与位置。
    pub(super) fn apply_init(&self, sci: &Sci, id: u32, init: DocInit) {
        let (lang_idx, long) = self
            .with_doc(id, |d| (d.lang, d.long_lines))
            .unwrap_or((0, false));
        if let Some(e) = init.eol {
            let mode = match e {
                Eol::Crlf => SC_EOL_CRLF,
                Eol::Lf => SC_EOL_LF,
                Eol::Cr => SC_EOL_CR,
            };
            sci.send(SCI_SETEOLMODE, mode as usize, 0);
        }
        let l = &LANGS[lang_idx];
        theme::set_language(
            sci,
            if long { "text" } else { l.id },
            if long { "null" } else { l.lexer },
        );
        sci.empty_undo_buffer();
        sci.set_save_point();
        sci.send(SCI_SETREADONLY, init.read_only as usize, 0);
        self.apply_doc_view();
        let lines = sci.line_count();
        for b in init.bookmarks.iter().filter(|&&b| b < lines) {
            sci.send(SCI_MARKERADD, *b, theme::MARK_BOOKMARK as isize);
        }
        for f in init.folds.iter().filter(|&&f| f < lines) {
            sci.send(SCI_FOLDLINE, *f, SC_FOLDACTION_CONTRACT);
        }
        if let Some(v) = init.view {
            let len = sci.len();
            sci.set_sel(v.anchor.min(len), v.caret.min(len));
            sci.send(SCI_SETFIRSTVISIBLELINE, v.first_line, 0);
        }
    }

    // ---- 会话强杀测试 ----

    pub(super) fn session_test_start(&self) {
        let Some((phase, marker, out)) = self.session_test.clone() else {
            return;
        };
        let Some(sci) = self.sci() else { return };
        match phase.as_str() {
            "write" => {
                self.new_document();
                sci.send_str(SCI_REPLACESEL, 0, &marker);
                self.mark_changed();
                unsafe { SetTimer(self.hwnd(), TIMER_SESSIONTEST, 4000, None) };
            }
            "check" => {
                let mut found = false;
                for id in self.doc_ids() {
                    self.activate(id);
                    if self.active.get() == id
                        && String::from_utf8_lossy(&sci.bytes()).contains(&marker)
                    {
                        found = true;
                    }
                }
                let count = self.doc_ids().len();
                // 清空会话，为下一轮做准备
                for id in self.doc_ids() {
                    self.with_doc_mut(id, |d| {
                        d.meta_dirty = false;
                        d.modified = false;
                    });
                    if self.active.get() == id {
                        sci.set_save_point();
                    }
                    self.close_doc_unchecked(id);
                }
                self.flush_persist();
                let _ = std::fs::write(&out, format!("found={}\ndocs={count}\n", found as u8));
                unsafe { DestroyWindow(self.hwnd()) };
            }
            _ => {}
        }
    }

    pub(super) fn session_test_ready(&self) {
        unsafe { KillTimer(self.hwnd(), TIMER_SESSIONTEST) };
        if let Some((_, _, out)) = &self.session_test {
            let _ = std::fs::write(out, "ready\n");
        }
    }
}
