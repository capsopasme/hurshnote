//! 目录搜索与目录替换（`app` 的子模块）。
//!
//! - 搜索在后台线程用 `hn_search::dir::search_dir` 执行（ignore 并行遍历 + 编码嗅探 + PCRE2），
//!   结果按批经 channel + `PostMessage(WM_APP_DIRSEARCH)` 回到 UI 线程，追加到抽屉的搜索结果
//! - 替换分两步：先按同样条件搜索（结果列表即预览），结束后确认，再逐个文件替换：
//!   已在编辑器中打开的文件在缓冲区中替换（可撤销，需要保存）；其余文件按原编码重新编码后
//!   走 `fsio::save_file` 安全写回；解码有损或有字符无法用原编码表示的文件跳过

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::{Duration, Instant};

use hn_sci::theme::out_style;
use hn_search::dir::{self as hdir, DirQuery, FileHits, Progress};

use super::*;
use crate::dirdlg::{self, DirAction, DirForm};
use drawer::{Page, ResultItem};

pub(super) const WM_APP_DIRSEARCH: u32 = WM_APP + 11;

pub(super) enum DirMsg {
    Batch {
        gen: u32,
        files: Vec<FileHits>,
        progress: Progress,
    },
    Done {
        gen: u32,
        result: Result<Progress, String>,
    },
}

/// 正在进行（或刚结束）的目录搜索。
pub(super) struct DirJob {
    cancel: Arc<AtomicBool>,
    query: DirQuery,
    /// 替换模板（替换任务）
    replace: Option<String>,
    files: Vec<FileHits>,
    started: Instant,
    progress: Progress,
    pub done: bool,
}

fn summary(p: &Progress, secs: f64) -> String {
    let mut s = format!(
        "{} 个文件中共 {} 处匹配（搜索了 {} 个文件，{:.2} 秒）",
        p.files_matched, p.matches, p.files_scanned, secs
    );
    let mut skipped = Vec::new();
    if p.skipped_binary > 0 {
        skipped.push(format!("二进制 {}", p.skipped_binary));
    }
    if p.skipped_large > 0 {
        skipped.push(format!("过大 {}", p.skipped_large));
    }
    if p.skipped_encrypted > 0 {
        skipped.push(format!("加密 {}", p.skipped_encrypted));
    }
    if !skipped.is_empty() {
        s.push_str(&format!("；跳过 {}", skipped.join("、")));
    }
    s
}

impl App {
    /// Ctrl+Shift+F：显示"在文件中查找"。
    pub(super) fn show_dir_dialog(&self) {
        if self.dir_dlg.get().is_null() {
            self.dir_dlg
                .set(dirdlg::create(self.hwnd(), self.hinstance));
        }
        let mut f = self.dir_form.borrow().clone();
        if f.dir.is_empty() {
            let roots = self.settings.borrow().workspace.clone();
            f.dir = if roots.is_empty() {
                self.active_doc(|d| d.path.clone())
                    .flatten()
                    .and_then(|p| p.parent().map(|d| d.display().to_string()))
                    .unwrap_or_default()
            } else {
                roots
                    .iter()
                    .map(|r| r.display().to_string())
                    .collect::<Vec<_>>()
                    .join(";")
            };
        }
        if let Some(sel) = self.selected_word() {
            f.pattern = sel;
        }
        dirdlg::show(self.dir_dlg.get(), self.hwnd(), &f);
    }

    /// 单行选区（用作默认搜索词）。
    fn selected_word(&self) -> Option<String> {
        if self.active_overlay().is_some() {
            return None;
        }
        let s = self.sci()?;
        let (a, b) = (s.selection_start(), s.selection_end());
        if a == b || b - a > 200 {
            return None;
        }
        let t = String::from_utf8(s.text_range(a, b)).ok()?;
        (!t.contains(['\r', '\n'])).then_some(t)
    }

    pub(crate) fn dir_action(&self, a: DirAction, f: DirForm) {
        match a {
            DirAction::Browse => {
                let owner = self.dir_dlg.get();
                if let Some(p) = self.browse_folder(owner, "选择要搜索的文件夹") {
                    let mut f = f;
                    f.dir = p.display().to_string();
                    dirdlg::load(owner, &f);
                    *self.dir_form.borrow_mut() = f;
                }
            }
            DirAction::Stop => self.cancel_dir_search(),
            DirAction::Find => self.start_dir_search(f, None),
            DirAction::Replace => {
                let t = f.replace.clone();
                self.start_dir_search(f, Some(t));
            }
        }
    }

    fn dir_status(&self, text: &str) {
        dirdlg::set_status(self.dir_dlg.get(), text);
        self.set_drawer_info(text);
    }

    pub(super) fn cancel_dir_search(&self) {
        if let Some(j) = self.dir_job.borrow().as_ref() {
            if !j.done {
                j.cancel.store(true, Ordering::Relaxed);
            }
        }
    }

    /// 开始搜索；`replace` 为替换模板时，搜索结束后确认并替换。
    pub(super) fn start_dir_search(&self, f: DirForm, replace: Option<String>) {
        if f.pattern.is_empty() {
            self.dir_status("请输入查找内容");
            return;
        }
        let roots = f.roots();
        if roots.is_empty() || roots.iter().any(|r| !r.is_dir()) {
            self.dir_status("目录不存在");
            return;
        }
        {
            let mut s = self.settings.borrow_mut();
            if s.dir_include != f.include || s.dir_exclude != f.exclude {
                s.dir_include = f.include.clone();
                s.dir_exclude = f.exclude.clone();
                config::save_settings(&s);
            }
        }
        *self.dir_form.borrow_mut() = f.clone();
        let mut q = DirQuery::new(roots, &f.pattern, f.options());
        q.include = hdir::split_globs(&f.include);
        q.exclude = hdir::split_globs(&f.exclude);
        q.gitignore = f.gitignore;
        q.hidden = f.hidden;
        q.hints = self.hints;
        if let Err(e) = Searcher::new(&q.pattern, &q.opts) {
            self.dir_status(&e.to_string());
            return;
        }
        self.cancel_dir_search();
        let gen = self.dir_gen.get().wrapping_add(1);
        self.dir_gen.set(gen);
        let cancel = Arc::new(AtomicBool::new(false));
        *self.dir_job.borrow_mut() = Some(DirJob {
            cancel: cancel.clone(),
            query: q.clone(),
            replace,
            files: Vec::new(),
            started: Instant::now(),
            progress: Progress::default(),
            done: false,
        });
        self.results_begin(0, gen);
        self.show_page(Page::Results, false);
        self.dir_status("正在搜索…");
        let tx = self.dir_tx.clone();
        let hwnd = self.hwnd() as isize;
        let pending = self.dir_pending.clone();
        let work = move || run_dir_search(q, gen, cancel, tx, hwnd, pending);
        if self.selftest_out.is_some() {
            // 自测：同步执行，结果仍经消息处理函数
            work();
            self.on_dir_msgs();
        } else {
            let _ = std::thread::Builder::new()
                .name("dir-search".into())
                .spawn(work);
        }
    }

    /// 处理搜索线程送来的结果。
    pub(super) fn on_dir_msgs(&self) {
        self.dir_pending.store(false, Ordering::SeqCst);
        let mut finished = false;
        while let Ok(m) = self.dir_rx.try_recv() {
            let current = self.dir_gen.get();
            match m {
                DirMsg::Batch {
                    gen,
                    files,
                    progress,
                } if gen == current => {
                    let items = Self::items_of(&files);
                    if let Some(j) = self.dir_job.borrow_mut().as_mut() {
                        j.progress = progress;
                        j.files.extend(files);
                    }
                    if !items.is_empty() {
                        self.results_push(items);
                    }
                    let text = format!(
                        "正在搜索…已搜索 {} 个文件，{} 处匹配",
                        progress.files_scanned, progress.matches
                    );
                    self.dir_status(&text);
                }
                DirMsg::Done { gen, result } if gen == current => {
                    self.finish_dir_search(result);
                    finished = true;
                }
                _ => {}
            }
        }
        if finished {
            self.maybe_replace();
        }
    }

    fn items_of(files: &[FileHits]) -> Vec<ResultItem> {
        let mut items = Vec::new();
        for f in files {
            let path = Rc::new(f.path.clone());
            let enc = if f.encoding == EncodingInfo::UTF8 {
                String::new()
            } else {
                f.encoding.label()
            };
            items.push(ResultItem::File {
                path: path.clone(),
                count: f.count,
                enc,
            });
            for h in &f.hits {
                items.push(ResultItem::Hit {
                    path: path.clone(),
                    line: h.line,
                    col: h.col,
                    col_end: h.col_end,
                    text: h.text.clone(),
                });
            }
        }
        items
    }

    fn finish_dir_search(&self, result: Result<Progress, String>) {
        let (text, files) = {
            let mut jb = self.dir_job.borrow_mut();
            let Some(j) = jb.as_mut() else { return };
            j.done = true;
            let secs = j.started.elapsed().as_secs_f64();
            let cancelled = j.cancel.load(Ordering::Relaxed);
            // 结果按路径排序，便于浏览
            j.files.sort_by(|a, b| a.path.cmp(&b.path));
            let text = match &result {
                Err(e) => e.clone(),
                Ok(p) if cancelled => format!("已停止。{}", summary(p, secs)),
                Ok(p) => {
                    j.progress = *p;
                    summary(p, secs)
                }
            };
            if result.is_err() || cancelled {
                j.replace = None;
            }
            (text, Self::items_of(&j.files))
        };
        let gen = self.dir_gen.get();
        self.results_begin(0, gen);
        self.results_push(files);
        self.dir_status(&text);
    }

    /// 替换任务：搜索结束后确认并执行。
    fn maybe_replace(&self) {
        let job = {
            let mut jb = self.dir_job.borrow_mut();
            let Some(j) = jb.as_mut() else { return };
            let Some(t) = j.replace.take() else { return };
            (t, j.query.clone(), j.files.clone(), j.progress)
        };
        let (template, q, files, p) = job;
        if files.is_empty() {
            self.dir_status("没有找到匹配，无需替换");
            return;
        }
        if self.selftest_out.is_none() {
            let text = format!(
                "将在 {} 个文件中替换 {} 处为“{}”。\n\n\
                 已在编辑器中打开的文件会在编辑器中修改（可撤销，需要保存）；\
                 其余文件直接写回磁盘（保留原编码）。\n\n确定继续？",
                p.files_matched, p.matches, template
            );
            if !dialogs::ask_ok_cancel(self.dir_dlg.get(), &text) {
                self.dir_status("已取消替换");
                return;
            }
        }
        self.apply_dir_replace(&q, &template, &files);
    }

    fn apply_dir_replace(&self, q: &DirQuery, template: &str, files: &[FileHits]) {
        let Ok(searcher) = Searcher::new(&q.pattern, &q.opts) else {
            return;
        };
        let original = self.active.get();
        let (mut n_files, mut n_matches) = (0usize, 0usize);
        let mut problems: Vec<(PathBuf, String)> = Vec::new();
        for f in files {
            let open = self
                .docs
                .borrow()
                .iter()
                .find(|d| {
                    d.path
                        .as_deref()
                        .is_some_and(|p| config::same_path(p, &f.path))
                })
                .map(|d| (d.id, d.is_large));
            match open {
                Some((_, true)) => problems.push((f.path.clone(), "以大文件视图打开，跳过".into())),
                Some((id, false)) => {
                    self.activate(id);
                    let Some(sci) = self.sci() else { continue };
                    if sci.is_read_only() {
                        problems.push((f.path.clone(), "文档为只读模式，跳过".into()));
                        continue;
                    }
                    let text = String::from_utf8_lossy(&sci.bytes()).into_owned();
                    match hdir::replace_in_text(&searcher, &text, template) {
                        Ok((new, n)) if n > 0 => {
                            let caret = sci.current_pos();
                            sci.undo_group(|| sci.replace_range(0, sci.len(), new.as_bytes()));
                            sci.set_sel(caret.min(sci.len()), caret.min(sci.len()));
                            n_files += 1;
                            n_matches += n;
                        }
                        Ok(_) => {}
                        Err(e) => problems.push((f.path.clone(), e.to_string())),
                    }
                }
                None => {
                    let bytes = match std::fs::read(&f.path) {
                        Ok(b) => b,
                        Err(e) => {
                            problems.push((f.path.clone(), e.to_string()));
                            continue;
                        }
                    };
                    match hdir::replace_in_bytes(&bytes, &searcher, template, &q.hints) {
                        Ok(Some((data, n))) => match crate::fsio::save_file(&f.path, &data) {
                            Ok(_) => {
                                n_files += 1;
                                n_matches += n;
                            }
                            Err(e) => problems.push((f.path.clone(), e.to_string())),
                        },
                        Ok(None) => {}
                        Err(e) => problems.push((f.path.clone(), e)),
                    }
                }
            }
        }
        if self.with_doc(original, |_| ()).is_some() {
            self.activate(original);
        }
        let mut text = format!("已在 {n_files} 个文件中替换 {n_matches} 处");
        if !problems.is_empty() {
            text.push_str(&format!("，{} 个文件未替换（见输出面板）", problems.len()));
            self.output_clear();
            self.output_append(&format!("目录替换：{text}\n"), out_style::INFO);
            for (p, why) in &problems {
                self.output_append(&format!("  {}：{why}\n", p.display()), out_style::ERROR);
            }
            self.show_page(Page::Output, false);
        }
        self.dir_status(&text);
        self.refresh();
    }

    // ---- 自测 ----

    pub(crate) fn st_dir_search(&self, f: DirForm, replace: bool) -> String {
        let t = replace.then(|| f.replace.clone());
        self.start_dir_search(f, t);
        dlg_text(self.drawer_info.get())
    }

    /// 目录搜索结果：(文件名, 行号 1 起始或 0 表示文件行, 文字)。
    pub(crate) fn st_dir_results(&self) -> Vec<(String, u64, String)> {
        self.results_items
            .borrow()
            .iter()
            .filter_map(|it| match it {
                ResultItem::File { path, count, .. } => Some((
                    path.file_name()?.to_string_lossy().into_owned(),
                    0,
                    count.to_string(),
                )),
                ResultItem::Hit {
                    path, line, text, ..
                } => Some((
                    path.file_name()?.to_string_lossy().into_owned(),
                    line + 1,
                    text.clone(),
                )),
                _ => None,
            })
            .collect()
    }

    pub(crate) fn st_jump_result(&self, i: usize) {
        self.jump_to_result(i);
    }
}

fn dlg_text(h: HWND) -> String {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowTextLengthW, GetWindowTextW};
    if h.is_null() {
        return String::new();
    }
    let len = unsafe { GetWindowTextLengthW(h) }.max(0) as usize;
    let mut buf = vec![0u16; len + 1];
    let n = unsafe { GetWindowTextW(h, buf.as_mut_ptr(), buf.len() as i32) }.max(0) as usize;
    String::from_utf16_lossy(&buf[..n])
}

fn run_dir_search(
    q: DirQuery,
    gen: u32,
    cancel: Arc<AtomicBool>,
    tx: Sender<DirMsg>,
    hwnd: isize,
    pending: Arc<AtomicBool>,
) {
    let wake = || {
        if !pending.swap(true, Ordering::SeqCst) {
            unsafe { PostMessageW(hwnd as HWND, WM_APP_DIRSEARCH, 0, 0) };
        }
    };
    let emit = |files: Vec<FileHits>, progress: Progress| {
        let _ = tx.send(DirMsg::Batch {
            gen,
            files,
            progress,
        });
        wake();
    };
    let result = hdir::search_dir(&q, &cancel, Duration::from_millis(100), &emit);
    let _ = tx.send(DirMsg::Done { gen, result });
    // Done 必须触发处理：先清掉标志再投递
    pending.store(false, Ordering::SeqCst);
    wake();
}
