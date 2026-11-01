//! 大文件视图、Hex 视图、Tail 模式与大文件搜索（`app` 的子模块；结果列在底部抽屉中）。
//!
//! - 超过阈值（256 MB）的文件、用户选择以 Hex 打开的二进制文件，以"大文件文档"打开：
//!   `Doc.is_large` 为真，`Doc.large` 持有状态，Scintilla 中挂一个空的只读文档占位
//! - 普通文档按 Ctrl+Shift+H 时，`Doc.large` 临时持有编码后内容的 Hex 快照（`embedded`）
//! - 有 `Doc.large` 的文档显示时，对应编辑视图上的大文件视图窗口盖住 Scintilla
//! - 后台线程（行索引、搜索）通过 channel + `WM_APP_LARGE` 回传，按代号（gen）丢弃过期结果
//! - 每 0.5 秒轮询大文件的磁盘状态：增长时继续建索引（Tail 时滚到末尾），截断/替换/改写时重新读取

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use hn_large::index::{self, IndexUpdate};
use hn_large::search::{self as lsearch, Walk};
use hn_large::tail::{Change, FileState};
use hn_large::{decode, ByteSource, Charset};
use hn_search::Searcher;
use hn_text::encoding::TextEncoding;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetFocus;

use super::*;
use crate::docs::DocInit;
use crate::lview::{self, LargeDoc, Mode};

pub(super) const WM_APP_LARGE: u32 = WM_APP + 9;
pub(super) const TIMER_LARGE: usize = 5;
const POLL_MS: u32 = 500;
/// 结果列表最多保留的条数（计数不受限制）。
const MAX_HITS: usize = 100_000;
const PREVIEW_CHARS: usize = 300;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
/// 大于此大小的文件用大文件视图打开。
pub(super) const LARGE_THRESHOLD: u64 = 256 * 1024 * 1024;

pub(super) enum LargeMsg {
    Index {
        doc: u32,
        gen: u32,
        up: IndexUpdate,
    },
    /// 行索引线程结束（无论成功与否）
    IndexEnd {
        doc: u32,
        gen: u32,
    },
    Search {
        doc: u32,
        gen: u32,
        ev: SearchEvent,
    },
}

pub(super) enum SearchEvent {
    Progress(f32),
    /// 查找上一个/下一个的结果：(匹配范围, 是否绕回)
    Found(Option<(u64, u64)>, bool),
    Hits(Vec<Hit>),
    Done {
        count: u64,
        truncated: bool,
        cancelled: bool,
        error: Option<String>,
    },
}

/// 一条搜索结果。
#[derive(Debug, Clone)]
pub(super) struct Hit {
    /// 行号（0 起始）；搜索不是从文件开头开始时未知
    pub line: Option<u64>,
    pub start: u64,
    pub end: u64,
    pub preview: String,
}

enum SearchKind {
    /// 从 `scan_from`（行首）开始扫描，取第一个起点不小于 `from` 的匹配
    Next {
        from: u64,
        scan_from: u64,
        wrap: bool,
    },
    /// 扫描到 `scan_to`（行尾），取最后一个起点小于 `before` 的匹配
    Prev {
        before: u64,
        scan_to: u64,
        wrap: bool,
    },
    /// 列出（或只统计）范围内的全部匹配
    All { range: (u64, u64), list: bool },
}

struct SearchJob {
    doc: u32,
    gen: u32,
    src: Arc<dyn ByteSource>,
    cs: Charset,
    enc: TextEncoding,
    data_start: u64,
    pattern: String,
    opts: hn_search::Options,
    kind: SearchKind,
    cancel: Arc<AtomicBool>,
}

/// 在文本中依次找匹配。回调返回 false 时停止；返回值表示是否遍历完。
fn iter_text(
    searcher: &Searcher,
    text: &str,
    f: &mut dyn FnMut(usize, usize) -> bool,
) -> Result<bool, String> {
    let len = text.len();
    let mut pos = 0;
    while pos <= len {
        match searcher
            .find_forward(text, pos, len)
            .map_err(|e| e.to_string())?
        {
            Some((s, e)) => {
                if !f(s, e) {
                    return Ok(false);
                }
                pos = if e > s {
                    e
                } else {
                    match text[e..].chars().next() {
                        Some(c) => e + c.len_utf8(),
                        None => break,
                    }
                };
            }
            None => break,
        }
    }
    Ok(true)
}

/// 在一块数据中依次找匹配，回调参数为相对块首的源字节范围。
fn each_match(
    searcher: &Searcher,
    data: &[u8],
    cs: Charset,
    legacy: Option<decode::LegacyDecode>,
    f: &mut dyn FnMut(usize, usize) -> bool,
) -> Result<bool, String> {
    if cs == Charset::Utf8 {
        if let Ok(text) = std::str::from_utf8(data) {
            return iter_text(searcher, text, f);
        }
    }
    // 非 UTF-8 或含无效字节：逐行解码（带偏移表）再匹配
    for (ls, le, _) in lsearch::lines(data, cs) {
        let (text, map) = decode::decode_mapped(&data[ls..le], cs, legacy);
        let cont = iter_text(searcher, &text, &mut |s, e| {
            f(
                ls + decode::source_offset(&map, s) as usize,
                ls + decode::source_offset(&map, e) as usize,
            )
        })?;
        if !cont {
            return Ok(false);
        }
    }
    Ok(true)
}

/// 匹配所在行的预览文字。
fn preview(data: &[u8], s: usize, cs: Charset, legacy: Option<decode::LegacyDecode>) -> String {
    let unit = cs.unit() as usize;
    let ls = cs.rfind_lf(&data[..s]).map(|i| i + unit).unwrap_or(0);
    let mut le = cs.find_lf(&data[s..]).map(|i| s + i).unwrap_or(data.len());
    if le >= ls + unit && cs.is_cr(&data[le - unit..]) {
        le -= unit;
    }
    // 预览只取行的前一部分
    let le = le.min(ls + PREVIEW_CHARS * 4);
    let (text, _) = decode::decode_mapped(&data[ls..le], cs, legacy);
    text.chars()
        .take(PREVIEW_CHARS)
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// 搜索线程的公共参数。
struct Ctx<'a> {
    searcher: &'a Searcher,
    src: &'a dyn ByteSource,
    cs: Charset,
    legacy: Option<decode::LegacyDecode<'a>>,
    cancel: &'a AtomicBool,
    send: &'a dyn Fn(SearchEvent),
}

/// 从行首 `start` 向后扫描，返回第一个被接受的匹配。
/// `accept(起点)`：Some(true) 接受，Some(false) 跳过，None 停止。块起点到达 `stop_at` 时停止。
fn first_forward(
    cx: &Ctx,
    start: u64,
    stop_at: u64,
    accept: &dyn Fn(u64) -> Option<bool>,
) -> (Walk, Option<(u64, u64)>, Option<String>) {
    let len = cx.src.len();
    let total = len.saturating_sub(start).max(1);
    let mut found = None;
    let mut error = None;
    let mut last = Instant::now();
    let w = lsearch::forward(
        cx.src,
        cx.cs,
        start,
        None,
        len,
        lsearch::CHUNK,
        cx.cancel,
        &mut |c| {
            if c.offset >= stop_at {
                return false;
            }
            let mut stop = false;
            let r = each_match(cx.searcher, c.data, cx.cs, cx.legacy, &mut |s, e| {
                let abs = c.offset + s as u64;
                match accept(abs) {
                    Some(true) => {
                        found = Some((abs, c.offset + e as u64));
                        false
                    }
                    Some(false) => true,
                    None => {
                        stop = true;
                        false
                    }
                }
            });
            if let Err(e) = r {
                error = Some(e);
                return false;
            }
            if found.is_some() || stop {
                return false;
            }
            if last.elapsed() >= PROGRESS_INTERVAL {
                last = Instant::now();
                (cx.send)(SearchEvent::Progress(
                    ((c.offset - start) as f64 / total as f64) as f32,
                ));
            }
            true
        },
    )
    .unwrap_or(Walk::Completed);
    (w, found, error)
}

/// 从 `end` 向前扫描到文件开头，返回最后一个被接受的匹配。块终点不大于 `stop_below` 时停止。
fn last_backward(
    cx: &Ctx,
    data_start: u64,
    end: u64,
    stop_below: u64,
    accept: &dyn Fn(u64) -> bool,
) -> (Walk, Option<(u64, u64)>, Option<String>) {
    let total = end.saturating_sub(data_start).max(1);
    let mut found = None;
    let mut error = None;
    let mut last = Instant::now();
    let w = lsearch::backward(
        cx.src,
        cx.cs,
        data_start,
        end,
        lsearch::CHUNK,
        cx.cancel,
        &mut |c| {
            if c.offset + (c.data.len() as u64) <= stop_below {
                return false;
            }
            let mut best = None;
            let r = each_match(cx.searcher, c.data, cx.cs, cx.legacy, &mut |s, e| {
                let abs = c.offset + s as u64;
                if accept(abs) {
                    best = Some((abs, c.offset + e as u64));
                }
                true
            });
            if let Err(e) = r {
                error = Some(e);
                return false;
            }
            if best.is_some() {
                found = best;
                return false;
            }
            if last.elapsed() >= PROGRESS_INTERVAL {
                last = Instant::now();
                (cx.send)(SearchEvent::Progress(
                    ((end - c.offset) as f64 / total as f64) as f32,
                ));
            }
            true
        },
    )
    .unwrap_or(Walk::Completed);
    (w, found, error)
}

/// 执行一次大文件搜索（在后台线程调用；自测时在 UI 线程同步调用）。
fn run_search(job: SearchJob, tx: Sender<LargeMsg>, hwnd: isize) {
    let SearchJob {
        doc,
        gen,
        src,
        cs,
        enc,
        data_start,
        pattern,
        opts,
        kind,
        cancel,
    } = job;
    let send = |ev: SearchEvent| {
        let _ = tx.send(LargeMsg::Search { doc, gen, ev });
        unsafe { PostMessageW(hwnd as HWND, WM_APP_LARGE, 0, 0) };
    };
    let done = |count: u64, truncated: bool, cancelled: bool, error: Option<String>| {
        send(SearchEvent::Done {
            count,
            truncated,
            cancelled,
            error,
        })
    };
    let searcher = match Searcher::new(&pattern, &opts) {
        Ok(s) => s,
        Err(e) => {
            done(0, false, false, Some(e.to_string()));
            return;
        }
    };
    let legacy_enc = match enc {
        TextEncoding::Legacy(e) => Some(e),
        _ => None,
    };
    let dec = move |b: &[u8]| -> String {
        match legacy_enc {
            Some(e) => e.decode_without_bom_handling(b).0.into_owned(),
            None => "\u{FFFD}".into(),
        }
    };
    let legacy: Option<decode::LegacyDecode> = if legacy_enc.is_some() {
        Some(&dec)
    } else {
        None
    };
    let cx = Ctx {
        searcher: &searcher,
        src: &*src,
        cs,
        legacy,
        cancel: &cancel,
        send: &send,
    };
    let len = src.len();

    match kind {
        SearchKind::Next {
            from,
            scan_from,
            wrap,
        } => {
            let (mut w, mut found, mut error) =
                first_forward(&cx, scan_from, u64::MAX, &|abs| Some(abs >= from));
            let mut wrapped = false;
            if found.is_none() && error.is_none() && w != Walk::Cancelled && wrap {
                wrapped = true;
                (w, found, error) = first_forward(&cx, data_start, from, &|abs| {
                    if abs < from {
                        Some(true)
                    } else {
                        None
                    }
                });
            }
            if w == Walk::Cancelled {
                done(0, false, true, None);
            } else if error.is_some() {
                done(0, false, false, error);
            } else {
                send(SearchEvent::Found(found, wrapped && found.is_some()));
            }
        }
        SearchKind::Prev {
            before,
            scan_to,
            wrap,
        } => {
            let (mut w, mut found, mut error) =
                last_backward(&cx, data_start, scan_to, 0, &|abs| abs < before);
            let mut wrapped = false;
            if found.is_none() && error.is_none() && w != Walk::Cancelled && wrap {
                wrapped = true;
                (w, found, error) =
                    last_backward(&cx, data_start, len, before, &|abs| abs >= before);
            }
            if w == Walk::Cancelled {
                done(0, false, true, None);
            } else if error.is_some() {
                done(0, false, false, error);
            } else {
                send(SearchEvent::Found(found, wrapped && found.is_some()));
            }
        }
        SearchKind::All { range, list } => {
            let (from, to) = range;
            let total = to.saturating_sub(from).max(1);
            let first_line = (from == data_start).then_some(0u64);
            let mut count = 0u64;
            let mut listed = 0usize;
            let mut truncated = false;
            let mut batch: Vec<Hit> = Vec::new();
            let mut error: Option<String> = None;
            let mut last = Instant::now();
            let w = lsearch::forward(
                &*src,
                cs,
                from,
                first_line,
                to,
                lsearch::CHUNK,
                &cancel,
                &mut |c| {
                    let mut line = c.first_line;
                    let mut counted_to = 0usize;
                    let r = each_match(&searcher, c.data, cs, legacy, &mut |s, e| {
                        if c.offset + (s as u64) >= to {
                            return false;
                        }
                        count += 1;
                        if list {
                            if listed < MAX_HITS {
                                if let Some(l) = line.as_mut() {
                                    *l += cs.count_lf(&c.data[counted_to..s.max(counted_to)]);
                                }
                                counted_to = counted_to.max(s);
                                batch.push(Hit {
                                    line,
                                    start: c.offset + s as u64,
                                    end: c.offset + e as u64,
                                    preview: preview(c.data, s, cs, legacy),
                                });
                                listed += 1;
                            } else {
                                truncated = true;
                            }
                        }
                        true
                    });
                    if let Err(e) = r {
                        error = Some(e);
                        return false;
                    }
                    if last.elapsed() >= PROGRESS_INTERVAL {
                        last = Instant::now();
                        send(SearchEvent::Progress(
                            ((c.offset - from) as f64 / total as f64) as f32,
                        ));
                        if !batch.is_empty() {
                            send(SearchEvent::Hits(std::mem::take(&mut batch)));
                        }
                    }
                    true
                },
            )
            .unwrap_or(Walk::Completed);
            if !batch.is_empty() {
                send(SearchEvent::Hits(batch));
            }
            done(count, truncated, w == Walk::Cancelled, error);
        }
    }
}

impl App {
    // ---- 查询 ----

    pub(super) fn overlay_of(&self, id: u32) -> Option<Rc<RefCell<LargeDoc>>> {
        self.docs
            .try_borrow()
            .ok()?
            .iter()
            .find(|d| d.id == id)
            .and_then(|d| d.large.clone())
    }

    /// 焦点视图中文档的大文件状态（大文件文档或临时 Hex 视图）。
    pub(super) fn active_overlay(&self) -> Option<Rc<RefCell<LargeDoc>>> {
        self.overlay_of(self.active.get())
    }

    /// 显示 `doc` 的视图能完整显示的行数。
    fn vis_for(doc: &Rc<RefCell<LargeDoc>>) -> u64 {
        lview::views_showing(doc)
            .first()
            .map(|v| v.visible_rows())
            .unwrap_or(30)
    }

    /// 大文件视图的字体：(字体名, 像素字号, 是否显示行号)。
    fn large_font(&self) -> (String, f32, bool) {
        let s = self.settings.borrow();
        let px =
            s.font_size_x100 as f32 / 100.0 * s.zoom as f32 / 100.0 * self.dpi.get() as f32 / 72.0;
        (s.font_face.clone(), px, s.line_numbers)
    }

    pub(super) fn lview_index(&self, hwnd: HWND) -> Option<usize> {
        (0..2).find(|&i| lview::view(i).is_some_and(|v| v.hwnd() == hwnd))
    }

    /// 按各视图当前文档显示或隐藏大文件视图窗口。
    pub(super) fn sync_overlays(&self) {
        let (face, px, numbers) = self.large_font();
        for i in 0..2 {
            let Some(lv) = lview::view(i) else { continue };
            let in_use = i == 0 || self.split.get() != 0;
            let id = self.view_doc[i].get();
            let ov = if in_use { self.overlay_of(id) } else { None };
            let sci = self.views[i].get();
            match ov {
                Some(d) => {
                    lv.set_font(&face, px, numbers);
                    let was = lv.is_shown();
                    lv.show(id, d);
                    if !was {
                        if let Some(s) = sci {
                            let had = unsafe { GetFocus() } == s.hwnd();
                            unsafe { ShowWindow(s.hwnd(), SW_HIDE) };
                            if had {
                                unsafe { SetFocus(lv.hwnd()) };
                            }
                        }
                    }
                }
                None => {
                    if lv.is_shown() {
                        let had = unsafe { GetFocus() } == lv.hwnd();
                        lv.hide();
                        if in_use {
                            if let Some(s) = sci {
                                unsafe { ShowWindow(s.hwnd(), SW_SHOW) };
                                if had {
                                    unsafe { SetFocus(s.hwnd()) };
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // ---- 打开 ----

    /// 以大文件视图打开文件。
    pub(super) fn open_large(
        &self,
        path: &Path,
        hex: bool,
        tail: bool,
        forced: Option<TextEncoding>,
    ) -> Option<u32> {
        let t = Instant::now();
        let mut ld = match LargeDoc::open(path, forced, &self.hints) {
            Ok(d) => d,
            Err(e) => {
                dialogs::error_box(self.hwnd(), &format!("无法打开 {}\n\n{e}", path.display()));
                return None;
            }
        };
        lview::record("doc_open_ms", t);
        if hex {
            ld.mode = Mode::Hex;
        }
        ld.tail = tail;
        let pristine = self.pristine_untitled();
        let id = self.install_large(ld, path)?;
        let t = Instant::now();
        self.activate(id);
        lview::record("activate_ms", t);
        if let Some(p) = pristine {
            if p != id {
                self.close_doc_unchecked(p);
            }
        }
        self.add_recent(path);
        self.start_index(id);
        if let Some(rc) = self.overlay_of(id) {
            if tail {
                let vis = Self::vis_for(&rc);
                rc.borrow_mut().goto_end(vis);
            }
            lview::reveal(&rc);
        }
        self.arm_large_timer();
        self.arm_session_timer();
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        if !hex && size > self.large_threshold.get() {
            self.show_message(&format!(
                "文件有 {}，以只读的大文件视图打开",
                fmt_size(size)
            ));
        }
        self.refresh();
        Some(id)
    }

    fn install_large(&self, ld: LargeDoc, path: &Path) -> Option<u32> {
        let sci = self.sci()?;
        let ptr = sci.create_document();
        let id = self.alloc_id();
        let mut d = Doc::new(id, ptr);
        d.path = Some(path.to_path_buf());
        d.enc = ld.enc;
        d.binary = ld.binary;
        d.is_large = true;
        d.large = Some(Rc::new(RefCell::new(ld)));
        d.init = Some(DocInit {
            read_only: true,
            ..Default::default()
        });
        self.docs.borrow_mut().push(d);
        Some(id)
    }

    /// 会话恢复：载入上次以大文件视图打开的文档。
    pub(super) fn load_pending_large(
        &self,
        id: u32,
        path: &Path,
        sd: &hn_core::session::SessionDoc,
    ) -> bool {
        let forced = TextEncoding::from_id(&sd.encoding);
        let mut ld = match LargeDoc::open(path, forced, &self.hints) {
            Ok(d) => d,
            Err(e) => {
                dialogs::error_box(self.hwnd(), &format!("无法打开 {}\n\n{e}", path.display()));
                self.with_doc_mut(id, |d| d.pending = Some(sd.clone()));
                return false;
            }
        };
        ld.mode = if sd.hex { Mode::Hex } else { Mode::Text };
        ld.tail = sd.tail;
        let off = ld.snap(sd.offset.min(ld.len()));
        ld.caret = off;
        ld.anchor = off;
        ld.top = ld.nav().row_start(off);
        ld.hex_top = off / hn_large::hex::ROW;
        let Some(sci) = self.sci() else { return false };
        let ptr = sci.create_document();
        self.with_doc_mut(id, |d| {
            d.ptr = ptr;
            d.enc = ld.enc;
            d.binary = ld.binary;
            d.is_large = true;
            d.large = Some(Rc::new(RefCell::new(ld)));
            d.snapshot = None;
            d.meta_dirty = false;
            d.modified = false;
            d.init = Some(DocInit {
                read_only: true,
                ..Default::default()
            });
        });
        self.start_index(id);
        self.arm_large_timer();
        true
    }

    // ---- 后台行索引 ----

    pub(super) fn start_index(&self, id: u32) {
        let Some(rc) = self.overlay_of(id) else {
            return;
        };
        let job = {
            let mut d = rc.borrow_mut();
            if d.embedded || d.scanning || (d.index.is_done() && d.index.scanned() >= d.len()) {
                return;
            }
            d.scanning = true;
            let (b, c) = d.index.resume_point();
            (d.source(), d.cs, b, c, d.len(), d.cancel.clone(), d.gen)
        };
        let (src, cs, block, cum, len, cancel, gen) = job;
        let tx = self.large_tx.clone();
        let hwnd = self.hwnd() as isize;
        let spawned = std::thread::Builder::new()
            .name("line-index".into())
            .spawn(move || {
                // 后台 I/O 优先级：不和界面线程的读取抢磁盘
                unsafe {
                    windows_sys::Win32::System::Threading::SetThreadPriority(
                        windows_sys::Win32::System::Threading::GetCurrentThread(),
                        0x0001_0000, // THREAD_MODE_BACKGROUND_BEGIN
                    )
                };
                index::scan(&*src, cs, block, cum, len, &cancel, &mut |up| {
                    let _ = tx.send(LargeMsg::Index { doc: id, gen, up });
                    unsafe { PostMessageW(hwnd as HWND, WM_APP_LARGE, 0, 0) };
                });
                let _ = tx.send(LargeMsg::IndexEnd { doc: id, gen });
                unsafe { PostMessageW(hwnd as HWND, WM_APP_LARGE, 0, 0) };
            });
        if spawned.is_err() {
            rc.borrow_mut().scanning = false;
        }
    }

    /// 处理后台线程的结果。
    pub(super) fn on_large_msgs(&self) {
        let mut touched: Vec<u32> = Vec::new();
        let mut restart: Vec<u32> = Vec::new();
        while let Ok(m) = self.large_rx.try_recv() {
            match m {
                LargeMsg::Index { doc, gen, up } => {
                    if let Some(rc) = self.overlay_of(doc) {
                        let mut d = rc.borrow_mut();
                        if d.gen == gen {
                            d.index.apply(&up);
                        }
                    }
                    if !touched.contains(&doc) {
                        touched.push(doc);
                    }
                }
                LargeMsg::IndexEnd { doc, gen } => {
                    if let Some(rc) = self.overlay_of(doc) {
                        let mut d = rc.borrow_mut();
                        if d.gen == gen {
                            d.scanning = false;
                            // 扫描期间文件又增长了：继续
                            if d.index.is_done() && d.index.scanned() < d.len() {
                                d.index.mark_incomplete();
                                restart.push(doc);
                            }
                        }
                    }
                    if !touched.contains(&doc) {
                        touched.push(doc);
                    }
                }
                LargeMsg::Search { doc, gen, ev } => self.on_search_event(doc, gen, ev),
            }
        }
        for doc in restart {
            self.start_index(doc);
        }
        for doc in touched {
            if let Some(rc) = self.overlay_of(doc) {
                lview::repaint(&rc);
            }
        }
        self.update_status();
    }

    // ---- 磁盘状态轮询（增长、轮转、截断） ----

    pub(super) fn arm_large_timer(&self) {
        let any = self.docs.borrow().iter().any(|d| {
            d.large
                .as_ref()
                .is_some_and(|l| l.try_borrow().map(|l| !l.embedded).unwrap_or(true))
        });
        unsafe {
            if any {
                SetTimer(self.hwnd(), TIMER_LARGE, POLL_MS, None);
            } else {
                KillTimer(self.hwnd(), TIMER_LARGE);
            }
        }
    }

    pub(super) fn on_large_timer(&self) {
        let list: Vec<(u32, Rc<RefCell<LargeDoc>>)> = self
            .docs
            .borrow()
            .iter()
            .filter_map(|d| d.large.clone().map(|l| (d.id, l)))
            .filter(|(_, l)| !l.borrow().embedded)
            .collect();
        if list.is_empty() {
            unsafe { KillTimer(self.hwnd(), TIMER_LARGE) };
            return;
        }
        for (id, rc) in list {
            self.poll_large(id, &rc);
        }
    }

    fn poll_large(&self, id: u32, rc: &Rc<RefCell<LargeDoc>>) {
        let (path, state) = {
            let d = rc.borrow();
            (d.path.clone(), d.state)
        };
        let Some(path) = path else { return };
        let Some(state) = state else {
            rc.borrow_mut().state = FileState::read(&path).ok();
            return;
        };
        let name = self.with_doc(id, |d| d.title()).unwrap_or_default();
        let vis = Self::vis_for(rc);
        let (change, new) = state.check(&path);
        match change {
            Change::None => {
                if let Some(n) = new {
                    rc.borrow_mut().state = Some(n);
                }
            }
            Change::Grown { from, to } => {
                let (tail, hint) = {
                    let mut d = rc.borrow_mut();
                    d.state = new;
                    if let Some(f) = &d.file {
                        f.set_len(to);
                    }
                    d.reader.invalidate_from(from - from % hn_large::BLOCK);
                    d.index.mark_incomplete();
                    d.missing = false;
                    let hint = !d.tail && !d.grown_hint;
                    d.grown_hint = true;
                    (d.tail, hint)
                };
                self.with_doc_mut(id, |d| d.missing = false);
                self.start_index(id);
                if tail {
                    rc.borrow_mut().goto_end(vis);
                    lview::reveal(rc);
                } else {
                    lview::repaint(rc);
                    if hint && id == self.active.get() {
                        self.show_message(&format!(
                            "“{name}”正在增长，可在“视图”菜单开启 Tail 模式跟随末尾"
                        ));
                    }
                }
                self.update_status();
            }
            Change::Truncated | Change::Replaced | Change::Rewritten => {
                {
                    let mut d = rc.borrow_mut();
                    if let (Some(f), Some(n)) = (&d.file, new) {
                        f.set_len(n.len);
                    }
                    d.state = new;
                    d.missing = false;
                    d.reset();
                    if d.tail {
                        d.goto_end(vis);
                    }
                }
                self.with_doc_mut(id, |d| d.missing = false);
                self.start_index(id);
                lview::reveal(rc);
                let what = match change {
                    Change::Truncated => "被截断",
                    Change::Replaced => "被替换（日志轮转）",
                    _ => "被改写",
                };
                self.show_message(&format!("“{name}”{what}，已重新读取"));
                self.refresh();
            }
            Change::Missing => {
                let first = {
                    let mut d = rc.borrow_mut();
                    !std::mem::replace(&mut d.missing, true)
                };
                if first {
                    self.with_doc_mut(id, |d| d.missing = true);
                    self.show_message(&format!("“{name}”已在磁盘上被删除或移走"));
                    self.refresh();
                }
            }
        }
    }

    // ---- 命令 ----

    /// 焦点视图显示大文件视图时的命令处理。返回 true 表示已处理（或已拦截）。
    pub(super) fn large_command(&self, id: u16) -> bool {
        let Some(rc) = self.active_overlay() else {
            return false;
        };
        let embedded = rc.borrow().embedded;
        let readonly_msg = if embedded {
            "Hex 视图是只读的快照，按 Ctrl+Shift+H 返回文本视图"
        } else {
            "大文件视图是只读的"
        };
        match id {
            IDM_EDIT_COPY => {
                let r = rc.borrow().selected_text();
                match r {
                    Ok(t) => self.copy_text(&t),
                    Err(e) => self.show_message(&e),
                }
                true
            }
            IDM_EDIT_SELECT_ALL => {
                {
                    let mut d = rc.borrow_mut();
                    d.anchor = if d.mode == Mode::Hex { 0 } else { d.data_start };
                    d.caret = d.len();
                }
                lview::repaint(&rc);
                self.update_status();
                true
            }
            IDM_SEARCH_GOTO => {
                self.large_goto();
                true
            }
            IDM_SEARCH_SEL_NEXT | IDM_SEARCH_SEL_PREV => {
                let text = {
                    let d = rc.borrow();
                    let (a, b) = d.sel();
                    if a == b || b - a > 500 {
                        None
                    } else {
                        d.selected_text().ok()
                    }
                };
                match text.filter(|t| !t.contains(['\r', '\n'])) {
                    Some(t) => {
                        let mut st = self.find.borrow().clone();
                        st.pattern = t;
                        st.opts.mode = hn_search::Mode::Normal;
                        let a = if id == IDM_SEARCH_SEL_NEXT {
                            FindAction::Next
                        } else {
                            FindAction::Prev
                        };
                        self.find_action(a, st);
                    }
                    None => self.show_message("先选中要查找的文字（不超过一行）"),
                }
                true
            }
            IDM_SEARCH_CLEAR_MARKS => true,
            IDM_SEARCH_REPLACE => {
                self.show_find(false);
                self.show_message(readonly_msg);
                true
            }
            IDM_FILE_SAVE | IDM_FILE_SAVE_AS | IDM_FILE_SAVE_ENCRYPTED if !embedded => {
                self.show_message(readonly_msg);
                true
            }
            // 与编辑内容无关的命令照常执行
            IDM_FILE_NEW
            | IDM_FILE_OPEN
            | IDM_FILE_QUICK_OPEN
            | IDM_WS_ADD
            | IDM_FILE_SAVE_ALL
            | IDM_FILE_CLOSE
            | IDM_FILE_CLOSE_ALL
            | IDM_FILE_EXIT
            | IDM_FILE_SAVE
            | IDM_FILE_SAVE_AS
            | IDM_FILE_SAVE_ENCRYPTED
            | IDM_SET_SNAPSHOTS
            | IDM_SET_AUTO_RELOAD
            | IDM_SET_GITIGNORE
            | IDM_RECENT_CLEAR
            | IDM_SEARCH_FIND
            | IDM_SEARCH_NEXT
            | IDM_SEARCH_PREV
            | IDM_SEARCH_RESULTS
            | IDM_WIN_NEXT
            | IDM_WIN_PREV
            | IDM_HELP_ABOUT => false,
            IDM_VIEW_WRAP..=IDM_VIEW_TAIL => false,
            IDM_CTX_CLOSE..=IDM_CTX_REVEAL | IDM_TREE_OPEN..=IDM_TREE_REMOVE_ROOT => false,
            x if (IDM_ENC_REOPEN_BASE..IDM_ENC_REOPEN_BASE + 100).contains(&x) && !embedded => {
                false
            }
            x if (IDM_ZOOM_BASE..IDM_ZOOM_BASE + 100).contains(&x)
                || (IDM_WINDOW_BASE..IDM_WINDOW_BASE + IDM_WINDOW_MAX).contains(&x)
                || (IDM_RECENT_BASE..IDM_RECENT_BASE + IDM_RECENT_MAX).contains(&x) =>
            {
                false
            }
            _ => {
                self.show_message(readonly_msg);
                true
            }
        }
    }

    /// 跳转到行（文本模式）或 Hex 行。
    fn large_goto(&self) {
        let Some(rc) = self.active_overlay() else {
            return;
        };
        let (mode, cur, known, done, pct, rows) = {
            let d = rc.borrow();
            (
                d.mode,
                match d.mode {
                    Mode::Text => d.caret_line().map(|l| l + 1).unwrap_or(1),
                    Mode::Hex => d.caret / hn_large::hex::ROW + 1,
                },
                d.index.line_count(),
                d.index.is_done() && !d.scanning,
                d.index_progress().unwrap_or(1.0) * 100.0,
                hn_large::hex::row_count(d.len()),
            )
        };
        if mode == Mode::Text && !done {
            self.show_message(&format!(
                "行索引建立中（{pct:.0}%），目前可跳转到第 {known} 行以内"
            ));
        }
        let max = if mode == Mode::Hex { rows } else { known };
        let Some(line) = finddlg::ask_line(
            self.hwnd(),
            self.hinstance,
            cur as usize,
            max.min(usize::MAX as u64) as usize,
        ) else {
            return;
        };
        let line = line as u64;
        let target = {
            let d = rc.borrow();
            match mode {
                Mode::Hex => Some(((line - 1) * hn_large::hex::ROW).min(d.len())),
                Mode::Text => d.index.line_start(line - 1, &d.reader),
            }
        };
        match target {
            Some(off) => {
                let vis = Self::vis_for(&rc);
                rc.borrow_mut().select(off, off, vis);
                lview::reveal(&rc);
                self.update_status();
            }
            None => self.show_message(&format!("第 {line} 行尚未建立索引，请稍候再试")),
        }
    }

    /// 切换 Hex 视图。
    pub(super) fn toggle_hex(&self) {
        let id = self.active.get();
        if let Some(rc) = self.active_overlay() {
            if rc.borrow().embedded {
                // 关闭临时 Hex 视图，回到文本
                let pos = self.text_pos_for_bytes(&rc);
                self.with_doc_mut(id, |d| d.large = None);
                self.refresh();
                if let Some(s) = self.sci() {
                    s.set_sel(pos, pos);
                    s.send(SCI_SCROLLCARET, 0, 0);
                    unsafe { SetFocus(s.hwnd()) };
                }
                return;
            }
            {
                let mut d = rc.borrow_mut();
                d.mode = if d.mode == Mode::Hex {
                    Mode::Text
                } else {
                    Mode::Hex
                };
                let (c, a) = (d.caret, d.anchor);
                d.caret = d.snap(c);
                d.anchor = d.snap(a);
                d.x_off = 0.0;
                d.want_x = None;
            }
            lview::reveal(&rc);
            self.refresh();
            return;
        }
        // 普通文档：按当前编码编码后显示字节
        let Some(sci) = self.sci() else { return };
        let enc = self.active_doc(|d| d.enc).unwrap_or_default();
        let bytes = sci.bytes();
        let text = String::from_utf8_lossy(&bytes);
        let mut pos = sci.current_pos().min(text.len());
        while !text.is_char_boundary(pos) {
            pos -= 1;
        }
        let caret = encoding::encode(&text[..pos], enc).bytes.len() as u64;
        let data = encoding::encode(&text, enc).bytes;
        let mut ld = LargeDoc::from_bytes(data, enc);
        ld.caret = caret.min(ld.len());
        ld.anchor = ld.caret;
        let rc = Rc::new(RefCell::new(ld));
        self.with_doc_mut(id, |d| d.large = Some(rc.clone()));
        self.refresh();
        lview::reveal(&rc);
    }

    /// 临时 Hex 视图中光标字节偏移 → 文本中的位置（UTF-8 字节）。
    fn text_pos_for_bytes(&self, rc: &Rc<RefCell<LargeDoc>>) -> usize {
        let d = rc.borrow();
        let caret = d.caret.max(d.data_start);
        let mut buf = vec![0u8; caret as usize];
        let n = d.source().read_full(0, &mut buf).unwrap_or(0);
        buf.truncate(n);
        let body = &buf[(d.data_start as usize).min(buf.len())..];
        let keep = d.cs.floor_boundary(body);
        let mut full = buf[..d.data_start as usize].to_vec();
        full.extend_from_slice(&body[..keep]);
        encoding::decode_as(&full, d.enc.encoding).text.len()
    }

    /// 切换 Tail 模式。普通文档（已保存、未修改）会改用大文件视图显示。
    pub(super) fn toggle_tail(&self) {
        let id = self.active.get();
        let Some((is_large, path, encrypted, enc, from_text)) = self.active_doc(|d| {
            (
                d.is_large,
                d.path.clone(),
                d.key.is_some(),
                d.enc,
                d.from_text,
            )
        }) else {
            return;
        };
        if is_large {
            let Some(rc) = self.active_overlay() else {
                return;
            };
            let on = {
                let mut d = rc.borrow_mut();
                d.tail = !d.tail;
                d.tail
            };
            if on {
                let vis = Self::vis_for(&rc);
                rc.borrow_mut().goto_end(vis);
                self.start_index(id);
                self.show_message("Tail 模式：跟随文件末尾");
            } else {
                self.show_message("已关闭 Tail 模式");
                // 由普通文档切换来的、文件不大：回到编辑器
                let small = path
                    .as_ref()
                    .and_then(|p| std::fs::metadata(p).ok())
                    .is_some_and(|m| m.len() <= self.large_threshold.get());
                if let (Some(ro), true) = (from_text, small) {
                    self.revert_to_text(id, ro);
                    return;
                }
            }
            lview::reveal(&rc);
            self.arm_large_timer();
            self.refresh();
            return;
        }
        if self.active_overlay().is_some() {
            // 临时 Hex 视图：先回到文本
            self.toggle_hex();
        }
        let Some(path) = path else {
            self.show_message("未保存的文档不能使用 Tail 模式");
            return;
        };
        if encrypted {
            self.show_message("加密文档不支持 Tail 模式");
            return;
        }
        if self.is_dirty(id) {
            self.show_message("请先保存修改，再开启 Tail 模式");
            return;
        }
        let mut ld = match LargeDoc::open(&path, Some(enc.encoding), &self.hints) {
            Ok(d) => d,
            Err(e) => {
                dialogs::error_box(self.hwnd(), &format!("无法打开 {}\n\n{e}", path.display()));
                return;
            }
        };
        ld.tail = true;
        let rc = Rc::new(RefCell::new(ld));
        let ro = self.sci().is_some_and(|s| s.is_read_only());
        self.unwatch(id);
        self.with_doc_mut(id, |d| {
            d.large = Some(rc.clone());
            d.is_large = true;
            d.from_text = Some(ro);
        });
        if let Some(s) = self.sci() {
            // 隐藏的编辑器内容不允许修改
            s.send(SCI_SETREADONLY, 1, 0);
        }
        self.start_index(id);
        let vis = Self::vis_for(&rc);
        rc.borrow_mut().goto_end(vis);
        self.refresh();
        lview::reveal(&rc);
        self.arm_large_timer();
        self.show_message("Tail 模式：跟随文件末尾");
    }

    /// 由普通文档切换到大文件视图（Tail）后，回到编辑器并重新载入。
    fn revert_to_text(&self, id: u32, read_only: bool) {
        self.with_doc_mut(id, |d| {
            d.large = None;
            d.is_large = false;
            d.from_text = None;
        });
        self.refresh();
        if let Some(s) = self.sci() {
            s.send(SCI_SETREADONLY, read_only as usize, 0);
        }
        self.arm_large_timer();
        self.reload_doc(id);
    }

    /// "重新打开为"某编码（大文件视图）。
    pub(super) fn large_reopen(&self, enc: TextEncoding) {
        let Some(rc) = self.active_overlay() else {
            return;
        };
        if rc.borrow().embedded {
            return;
        }
        rc.borrow_mut().set_encoding(enc);
        let info = rc.borrow().enc;
        self.active_doc_mut(|d| d.enc = info);
        self.start_index(self.active.get());
        lview::reveal(&rc);
        self.refresh();
    }

    // ---- 搜索 ----

    /// 大文件视图中的查找（在后台线程执行）。
    pub(super) fn large_find(&self, action: FindAction, st: FindState) {
        let Some(rc) = self.active_overlay() else {
            return;
        };
        match action {
            FindAction::Replace | FindAction::ReplaceAll => {
                self.report("大文件视图是只读的，不能替换");
                return;
            }
            FindAction::ClearMarks => {
                self.report("");
                return;
            }
            _ => {}
        }
        if let Err(e) = Searcher::new(&st.pattern, &st.opts) {
            self.report(&e.to_string());
            return;
        }
        let doc = self.active.get();
        let kind = {
            let d = rc.borrow();
            let nav = d.nav();
            let (sa, sb) = d.sel();
            match action {
                FindAction::Next => SearchKind::Next {
                    from: if sa == sb { sb } else { sa + 1 }.max(d.data_start),
                    scan_from: nav.row_start(sa),
                    wrap: st.wrap,
                },
                FindAction::Prev => SearchKind::Prev {
                    before: sa,
                    scan_to: nav.row(nav.row_start(sb)).next.unwrap_or(d.len()),
                    wrap: st.wrap,
                },
                _ => SearchKind::All {
                    range: if st.in_selection && sa < sb {
                        (sa, sb)
                    } else {
                        (d.data_start, d.len())
                    },
                    list: action == FindAction::MarkAll,
                },
            }
        };
        let list = matches!(kind, SearchKind::All { list: true, .. });
        let job = {
            let mut d = rc.borrow_mut();
            if let Some(c) = d.search_cancel.take() {
                c.store(true, Ordering::Relaxed);
            }
            d.search_gen = d.search_gen.wrapping_add(1);
            let cancel = Arc::new(AtomicBool::new(false));
            d.search_cancel = Some(cancel.clone());
            d.search_progress = Some(0.0);
            SearchJob {
                doc,
                gen: d.search_gen,
                src: d.source(),
                cs: d.cs,
                enc: d.enc.encoding,
                data_start: d.data_start,
                pattern: st.pattern.clone(),
                opts: st.opts,
                kind,
                cancel,
            }
        };
        if list {
            self.results_begin(doc, job.gen);
            self.show_page(drawer::Page::Results, false);
        }
        self.report("正在搜索…（Esc 取消）");
        let tx = self.large_tx.clone();
        let hwnd = self.hwnd() as isize;
        if self.selftest_out.is_some() {
            // 自测：同步执行，结果仍经消息队列处理
            run_search(job, tx, hwnd);
            self.on_large_msgs();
        } else {
            let _ = std::thread::Builder::new()
                .name("large-search".into())
                .spawn(move || run_search(job, tx, hwnd));
        }
        self.update_status();
    }

    fn on_search_event(&self, doc: u32, gen: u32, ev: SearchEvent) {
        let Some(rc) = self.overlay_of(doc) else {
            return;
        };
        if rc.borrow().search_gen != gen {
            return;
        }
        let finish = |rc: &Rc<RefCell<LargeDoc>>| {
            let mut d = rc.borrow_mut();
            d.search_progress = None;
            d.search_cancel = None;
        };
        match ev {
            SearchEvent::Progress(p) => {
                rc.borrow_mut().search_progress = Some(p);
            }
            SearchEvent::Found(Some((a, b)), wrapped) => {
                finish(&rc);
                let vis = Self::vis_for(&rc);
                rc.borrow_mut().select(a, b, vis);
                lview::reveal(&rc);
                self.report(if wrapped {
                    "已从另一端继续查找"
                } else {
                    ""
                });
            }
            SearchEvent::Found(None, _) => {
                finish(&rc);
                let pat = self.find.borrow().pattern.clone();
                self.report(&format!("找不到“{pat}”"));
                unsafe { windows_sys::Win32::System::Diagnostics::Debug::MessageBeep(0) };
            }
            SearchEvent::Hits(v) => {
                if self.results_doc.get() == doc && self.results_gen.get() == gen {
                    self.results_push(
                        v.into_iter()
                            .map(|h| drawer::ResultItem::Large {
                                doc,
                                line: h.line,
                                start: h.start,
                                end: h.end,
                                preview: h.preview,
                            })
                            .collect(),
                    );
                }
            }
            SearchEvent::Done {
                count,
                truncated,
                cancelled,
                error,
            } => {
                finish(&rc);
                let listing = self.results_doc.get() == doc && self.results_gen.get() == gen;
                if let Some(e) = error {
                    self.report(&e);
                } else if cancelled {
                    self.report("已取消搜索");
                } else if listing {
                    self.report(&format!(
                        "共 {count} 处匹配{}",
                        if truncated {
                            "，搜索结果中只列出前 10 万处"
                        } else {
                            "，已列在搜索结果中（F7 显示/隐藏）"
                        }
                    ));
                } else {
                    self.report(&format!("共 {count} 处匹配"));
                }
            }
        }
        self.update_status();
    }

    /// Esc：取消正在进行的搜索。
    pub(super) fn cancel_large_search(&self) {
        if let Some(rc) = self.active_overlay() {
            let c = rc.borrow_mut().search_cancel.take();
            if let Some(c) = c {
                c.store(true, Ordering::Relaxed);
                rc.borrow_mut().search_progress = None;
                self.report("已取消搜索");
            }
        }
    }

    // ---- 状态栏与标题 ----

    /// 大文件视图的状态栏。
    pub(super) fn update_large_status(&self, sb: HWND, rc: &Rc<RefCell<LargeDoc>>) {
        let Ok(d) = rc.try_borrow() else { return };
        let msg = self.message.borrow().clone();
        let mut flags: Vec<String> = Vec::new();
        if d.embedded {
            flags.push("Hex 视图（只读快照）".into());
        } else {
            flags.push(format!("大文件视图 · 只读 · {}", fmt_size(d.len())));
        }
        if d.tail {
            flags.push("Tail".into());
        }
        if d.binary {
            flags.push("二进制".into());
        }
        if d.missing {
            flags.push("磁盘上已删除".into());
        }
        if let Some(p) = d.search_progress {
            flags.push(format!("正在搜索 {:.0}%（Esc 取消）", p * 100.0));
        } else if !d.embedded {
            if let Some(p) = d.index_progress() {
                flags.push(format!("正在建立行索引 {:.0}%", p * 100.0));
            }
        }
        let part0 = if msg.is_empty() {
            flags.join(" · ")
        } else {
            msg
        };
        let pos = match d.mode {
            Mode::Text => match d.caret_line() {
                Some(l) => format!("行 {}，偏移 {}", l + 1, d.caret),
                None => format!("偏移 {}", d.caret),
            },
            Mode::Hex => format!("偏移 0x{:X}（{}）", d.caret, d.caret),
        };
        let (a, b) = d.sel();
        let sel = if b > a {
            format!("选中 {} 字节", b - a)
        } else {
            String::new()
        };
        let lang = if d.mode == Mode::Hex {
            "Hex"
        } else {
            "大文件"
        };
        set(sb, PART_MESSAGE, &part0);
        set(sb, PART_POS, &pos);
        set(sb, PART_SEL, &sel);
        set(sb, PART_LANG, lang);
        set(sb, PART_EOL, d.eol);
        set(sb, PART_ENC, &d.enc.label());
        set(
            sb,
            PART_ZOOM,
            &format!(
                "{}%",
                self.settings.try_borrow().map(|s| s.zoom).unwrap_or(100)
            ),
        );
        set(sb, PART_STATE, "只读");
    }

    // ---- 大文件视图窗口的回调 ----

    pub(crate) fn lv_focus(&self, idx: usize) {
        if idx != self.focus.get() {
            unsafe { PostMessageW(self.hwnd(), WM_APP_FOCUS_VIEW, idx, 0) };
        }
    }

    pub(crate) fn lv_changed(&self) {
        self.update_status();
    }

    pub(crate) fn lv_ctrl_wheel(&self, delta: i32) {
        self.on_ctrl_wheel(delta);
    }

    pub(crate) fn lv_copy(&self, text: &str) {
        self.copy_text(text);
    }

    pub(crate) fn lv_message(&self, text: &str) {
        self.show_message(text);
    }

    pub(crate) fn lv_escape(&self) {
        self.cancel_large_search();
    }

    // ---- 自测 ----

    pub(crate) fn st_set_large_threshold(&self, n: u64) {
        self.large_threshold.set(n);
    }

    pub(crate) fn st_open(&self, path: &Path) {
        self.open_path(path, None, false);
    }

    pub(crate) fn st_open_large(&self, path: &Path, hex: bool, tail: bool) -> Option<u32> {
        self.open_large(path, hex, tail, None)
    }

    pub(crate) fn st_active_large(&self) -> Option<Rc<RefCell<LargeDoc>>> {
        self.active_overlay()
    }

    pub(crate) fn st_active_is_large(&self) -> bool {
        self.active_doc(|d| d.is_large).unwrap_or(false)
    }

    /// 处理后台消息直到行索引完成或超时。
    pub(crate) fn st_wait_index(&self, timeout_ms: u64) -> bool {
        let start = Instant::now();
        loop {
            self.on_large_msgs();
            let done = self.active_overlay().is_some_and(|rc| {
                let d = rc.borrow();
                d.index.is_done() && !d.scanning
            });
            if done {
                return true;
            }
            if start.elapsed().as_millis() as u64 > timeout_ms {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    pub(crate) fn st_poll_large(&self) {
        self.on_large_timer();
    }

    pub(crate) fn st_results(&self) -> Vec<(Option<u64>, u64, String)> {
        self.results_items
            .borrow()
            .iter()
            .filter_map(|h| match h {
                drawer::ResultItem::Large {
                    line,
                    start,
                    preview,
                    ..
                } => Some((*line, *start, preview.clone())),
                _ => None,
            })
            .collect()
    }

    pub(crate) fn st_toggle_hex(&self) {
        self.toggle_hex();
    }

    pub(crate) fn st_toggle_tail(&self) {
        self.toggle_tail();
    }

    pub(crate) fn st_large_goto_line(&self, line: u64) -> Option<u64> {
        let rc = self.active_overlay()?;
        let off = {
            let d = rc.borrow();
            d.index.line_start(line, &d.reader)?
        };
        let vis = Self::vis_for(&rc);
        rc.borrow_mut().select(off, off, vis);
        lview::reveal(&rc);
        Some(off)
    }

    pub(crate) fn st_focus_view_index(&self) -> usize {
        self.focus.get()
    }
}

/// 字节数的友好显示。
pub(super) fn fmt_size(n: u64) -> String {
    const K: f64 = 1024.0;
    let f = n as f64;
    if f >= K * K * K {
        format!("{:.2} GB", f / (K * K * K))
    } else if f >= K * K {
        format!("{:.1} MB", f / (K * K))
    } else if f >= K {
        format!("{:.1} KB", f / K)
    } else {
        format!("{n} 字节")
    }
}
