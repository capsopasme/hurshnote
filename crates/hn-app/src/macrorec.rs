//! 宏录制与回放（`app` 的子模块）。步骤模型与持久化在 `hn_core::macros`。
//!
//! 录制：两个编辑视图都 `SCI_STARTRECORD`，`SCN_MACRORECORD` 通知逐条记下；菜单命令（白名单内的编辑命令）
//! 与查找对话框的操作由程序自己记下。程序自己发给 Scintilla 的消息（列表续写、行操作的实现……）在执行期间
//! "静音"，否则会被重复录入。
//!
//! 回放：整个回放是一个撤销步骤；"运行到文件末尾"在光标到达末尾、或一轮下来光标与文档长度都没变时停止。
//!
//! 持久化：最近录制的宏保存到数据目录的 `macro.json`，重启后可继续回放；录制期间碰过加密文档的宏
//! 只留在内存里（其中可能有明文）。

use std::cell::Cell;

use hn_core::macros::{self, FindStep, Recorder, Step};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DialogBoxIndirectParamW, EndDialog, DLGTEMPLATE,
};

use super::*;
use crate::dlg::{self, Template};

const IDC_TIMES: u16 = 301;
const IDC_ONCE_N: u16 = 302;
const IDC_TO_END: u16 = 303;
/// "运行到文件末尾"最多运行的次数与时间
const MAX_RUNS: u32 = 1_000_000;
const MAX_SECONDS: u64 = 60;

/// 宏状态。
#[derive(Default)]
pub(super) struct MacroState {
    rec: Option<Recorder>,
    steps: Vec<Step>,
    /// 录制期间碰过加密文档
    sensitive: bool,
    /// 已尝试从磁盘读取
    loaded: bool,
    playing: bool,
}

/// 执行期间静音宏录制（程序自己发出的 Scintilla 消息不录）。
pub(super) struct Mute<'a>(&'a Cell<u32>);

impl Drop for Mute<'_> {
    fn drop(&mut self) {
        self.0.set(self.0.get().saturating_sub(1));
    }
}

/// 可以录进宏、回放时执行的菜单命令（只改动当前文档内容或光标的编辑命令）。
pub(super) fn recordable(id: u16) -> bool {
    matches!(
        id,
        IDM_EDIT_UNDO..=IDM_EDIT_SELECT_ALL
            | IDM_EDIT_UPPER
            | IDM_EDIT_LOWER
            | IDM_LINE_DUPLICATE..=IDM_LINE_TRIM_LEADING
            | IDM_EOL_CRLF..=IDM_EOL_CR
            | IDM_SEARCH_NEXT..=IDM_SEARCH_CLEAR_MARKS
            | IDM_BM_TOGGLE..=IDM_BM_DELETE_UNMARKED
    )
}

fn macro_path() -> PathBuf {
    config::data_dir().join("macro.json")
}

fn action_name(a: FindAction) -> &'static str {
    match a {
        FindAction::Next => "next",
        FindAction::Prev => "prev",
        FindAction::Count => "count",
        FindAction::MarkAll => "mark_all",
        FindAction::ClearMarks => "clear_marks",
        FindAction::Replace => "replace",
        FindAction::ReplaceAll => "replace_all",
    }
}

fn action_from(s: &str) -> Option<FindAction> {
    Some(match s {
        "next" => FindAction::Next,
        "prev" => FindAction::Prev,
        "count" => FindAction::Count,
        "mark_all" => FindAction::MarkAll,
        "clear_marks" => FindAction::ClearMarks,
        "replace" => FindAction::Replace,
        "replace_all" => FindAction::ReplaceAll,
        _ => return None,
    })
}

fn find_step(a: FindAction, st: &FindState) -> FindStep {
    FindStep {
        action: action_name(a).into(),
        pattern: st.pattern.clone(),
        replace: st.replace.clone(),
        mode: match st.opts.mode {
            hn_search::Mode::Normal => 0,
            hn_search::Mode::Extended => 1,
            hn_search::Mode::Regex => 2,
        },
        case_sensitive: st.opts.case_sensitive,
        whole_word: st.opts.whole_word,
        dot_all: st.opts.dot_all,
        wrap: st.wrap,
        in_selection: st.in_selection,
    }
}

fn find_state(f: &FindStep) -> FindState {
    FindState {
        pattern: f.pattern.clone(),
        replace: f.replace.clone(),
        opts: hn_search::Options {
            mode: match f.mode {
                1 => hn_search::Mode::Extended,
                2 => hn_search::Mode::Regex,
                _ => hn_search::Mode::Normal,
            },
            case_sensitive: f.case_sensitive,
            whole_word: f.whole_word,
            dot_all: f.dot_all,
        },
        wrap: f.wrap,
        in_selection: f.in_selection,
    }
}

impl App {
    pub(super) fn mute_macro(&self) -> Mute<'_> {
        self.macro_mute.set(self.macro_mute.get() + 1);
        Mute(&self.macro_mute)
    }

    pub(super) fn is_recording(&self) -> bool {
        self.macro_state
            .try_borrow()
            .map(|m| m.rec.is_some())
            .unwrap_or(false)
    }

    fn recording_unmuted(&self) -> bool {
        self.macro_mute.get() == 0 && self.is_recording()
    }

    /// 开始 / 停止录制。
    pub(super) fn toggle_record(&self) {
        if self.macro_state.borrow().playing {
            return;
        }
        let views: Vec<Sci> = self.views.iter().filter_map(|v| v.get()).collect();
        if !self.is_recording() {
            let sensitive = self.active_doc(|d| d.is_encrypted()).unwrap_or(false);
            {
                let mut m = self.macro_state.borrow_mut();
                m.rec = Some(Recorder::new());
                m.sensitive = sensitive;
            }
            for v in &views {
                v.send(SCI_STARTRECORD, 0, 0);
            }
            self.show_message("开始录制宏（Ctrl+Shift+R 停止）");
            return;
        }
        for v in &views {
            v.send(SCI_STOPRECORD, 0, 0);
        }
        let (steps, sensitive) = {
            let mut m = self.macro_state.borrow_mut();
            let rec = m.rec.take().unwrap_or_default();
            (rec.finish(), m.sensitive)
        };
        if steps.is_empty() {
            self.show_message("没有录到任何操作，保留原来的宏");
            self.update_status();
            return;
        }
        let n = steps.len();
        let path = macro_path();
        let note = if sensitive {
            // 可能含有加密文档的明文：不写盘，并删除旧文件，避免下次启动回放的是旧宏
            let _ = std::fs::remove_file(&path);
            "（录制时打开过加密文档，宏只保留在内存中）"
        } else {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = hn_core::atomic::write_atomic(&path, macros::to_json(&steps).as_bytes());
            ""
        };
        {
            let mut m = self.macro_state.borrow_mut();
            m.steps = steps;
            m.loaded = true;
        }
        self.show_message(&format!("已录制宏：{n} 步{note}，Ctrl+Shift+P 回放"));
    }

    /// `SCN_MACRORECORD`：记下一条 Scintilla 消息（必须在通知处理中立即读取字符串参数）。
    pub(super) fn on_macro_record(&self, n: &Notification) {
        if !self.recording_unmuted() {
            return;
        }
        let msg = n.message as u32;
        let text = unsafe { hn_sci::macro_text(msg, n.wparam, n.lparam) };
        // 字符串参数的 lParam 是指针，没有意义；其余消息记下原值
        let lp = if text.is_some() { 0 } else { n.lparam as i64 };
        let encrypted = self.active_doc(|d| d.is_encrypted()).unwrap_or(false);
        if let Ok(mut m) = self.macro_state.try_borrow_mut() {
            m.sensitive |= encrypted;
            if let Some(r) = m.rec.as_mut() {
                r.sci(msg, n.wparam as u64, lp, text, SCI_REPLACESEL);
            }
        }
    }

    /// 菜单命令执行前调用：录制中且可录制时记下。
    pub(super) fn record_command(&self, id: u16) {
        if !self.recording_unmuted() || !recordable(id) {
            return;
        }
        let encrypted = self.active_doc(|d| d.is_encrypted()).unwrap_or(false);
        if let Ok(mut m) = self.macro_state.try_borrow_mut() {
            m.sensitive |= encrypted;
            if let Some(r) = m.rec.as_mut() {
                r.command(id);
            }
        }
    }

    /// 查找对话框的操作。返回执行期间的静音守卫（录制时）。
    pub(super) fn record_find(&self, action: FindAction, st: &FindState) -> Option<Mute<'_>> {
        if !self.recording_unmuted() {
            return None;
        }
        if let Ok(mut m) = self.macro_state.try_borrow_mut() {
            // 查找内容本身也可能来自加密文档
            m.sensitive |= self.active_doc(|d| d.is_encrypted()).unwrap_or(false);
            if let Some(r) = m.rec.as_mut() {
                r.find(find_step(action, st));
            }
        }
        Some(self.mute_macro())
    }

    fn ensure_macro_loaded(&self) {
        let mut m = self.macro_state.borrow_mut();
        if m.loaded || !m.steps.is_empty() {
            return;
        }
        m.loaded = true;
        if let Ok(s) = std::fs::read_to_string(macro_path()) {
            if let Ok(steps) = macros::from_json(&s) {
                m.steps = steps;
            }
        }
    }

    /// 回放 `times` 次；`until_end` 时运行到文件末尾。
    pub(super) fn play_macro(&self, times: u32, until_end: bool) {
        if self.is_recording() {
            self.show_message("正在录制宏，请先停止录制（Ctrl+Shift+R）");
            return;
        }
        if self.macro_state.borrow().playing {
            return;
        }
        if self.active_overlay().is_some() {
            self.show_message("大文件视图与 Hex 视图中不能回放宏");
            return;
        }
        let Some(sci) = self.sci() else { return };
        if sci.is_read_only() {
            self.show_message("文档为只读，不能回放宏");
            return;
        }
        self.ensure_macro_loaded();
        let steps = self.macro_state.borrow().steps.clone();
        if steps.is_empty() {
            self.show_message("还没有录制宏（Ctrl+Shift+R 开始录制）");
            return;
        }
        self.macro_state.borrow_mut().playing = true;
        let started = std::time::Instant::now();
        let mut runs = 0u32;
        let mut timed_out = false;
        sci.undo_group(|| loop {
            let before = (sci.current_pos(), sci.len());
            for s in &steps {
                self.exec_step(&sci, s);
            }
            runs += 1;
            if started.elapsed().as_secs() >= MAX_SECONDS {
                timed_out = true;
                break;
            }
            if until_end {
                let after = (sci.current_pos(), sci.len());
                if after == before || after.0 >= after.1 || runs >= MAX_RUNS {
                    break;
                }
            } else if runs >= times.max(1) {
                break;
            }
        });
        self.macro_state.borrow_mut().playing = false;
        let msg = if timed_out {
            format!("回放超过 {MAX_SECONDS} 秒，已在第 {runs} 次后停止")
        } else if runs > 1 {
            format!("宏已回放 {runs} 次")
        } else {
            String::new()
        };
        self.show_message(&msg);
        self.refresh();
    }

    fn exec_step(&self, sci: &Sci, step: &Step) {
        match step {
            Step::Sci { msg, wp, lp, text } => match text {
                Some(t) if matches!(*msg, SCI_ADDTEXT | SCI_APPENDTEXT) => {
                    sci.send(*msg, t.len(), t.as_ptr() as isize);
                }
                Some(t) => {
                    sci.send_str(*msg, *wp as usize, t);
                }
                None => {
                    sci.send(*msg, *wp as usize, *lp as isize);
                }
            },
            // 像按下回车一样：触发自动缩进与列表续写
            Step::NewLine => {
                sci.send(SCI_NEWLINE, 0, 0);
            }
            Step::Command { id } => {
                // 宏文件可能被改过：只执行白名单内的命令
                if recordable(*id) {
                    self.command(*id);
                }
            }
            Step::Find(f) => {
                if let Some(a) = action_from(&f.action) {
                    self.find_action(a, find_state(f));
                }
            }
        }
    }

    /// 宏 → 多次回放…
    pub(super) fn play_macro_dialog(&self) {
        TIMES.with(|c| c.set((1, false)));
        let t = Template::new(
            "多次回放宏",
            WS_POPUP | WS_CAPTION | WS_SYSMENU | DS_MODALFRAME | DS_CENTER,
            200,
            84,
        )
        .radio("运行次数：", IDC_ONCE_N, 7, 9, 70, 10, true)
        .edit(IDC_TIMES, 80, 7, 60, 13, ES_NUMBER)
        .radio("运行到文件末尾", IDC_TO_END, 7, 28, 180, 10, false)
        .button("运行", IDOK as u16, 86, 62, 50, 14, true)
        .button("取消", IDCANCEL as u16, 143, 62, 50, 14, false)
        .build();
        let r = unsafe {
            DialogBoxIndirectParamW(
                self.hinstance,
                t.as_ptr() as *const DLGTEMPLATE,
                self.hwnd(),
                Some(times_proc),
                0,
            )
        };
        if r == IDOK as isize {
            let (n, to_end) = TIMES.with(|c| c.get());
            self.play_macro(n, to_end);
        }
    }

    // ---- 自测 ----

    pub(crate) fn st_macro_steps(&self) -> usize {
        self.macro_state.borrow().steps.len()
    }

    pub(crate) fn st_play_macro(&self, times: u32, until_end: bool) {
        self.play_macro(times, until_end);
    }

    pub(crate) fn st_toggle_record(&self) {
        self.toggle_record();
    }
}

thread_local! {
    static TIMES: Cell<(u32, bool)> = const { Cell::new((1, false)) };
}

unsafe extern "system" fn times_proc(h: HWND, msg: u32, wp: WPARAM, _lp: LPARAM) -> isize {
    let r = crate::crash::guard(|| match msg {
        WM_INITDIALOG => {
            dlg::set_text(h, IDC_TIMES, "10");
            dlg::set_checked(h, IDC_ONCE_N, true);
            dlg::select_all(h, IDC_TIMES);
            1
        }
        WM_COMMAND => match loword(wp) as i32 {
            IDOK => {
                let n = dlg::get_text(h, IDC_TIMES)
                    .trim()
                    .parse::<u32>()
                    .unwrap_or(1)
                    .clamp(1, MAX_RUNS);
                let to_end = dlg::is_checked(h, IDC_TO_END);
                TIMES.with(|c| c.set((n, to_end)));
                unsafe { EndDialog(h, IDOK as isize) };
                1
            }
            IDCANCEL => {
                unsafe { EndDialog(h, IDCANCEL as isize) };
                1
            }
            _ => 0,
        },
        _ => 0,
    });
    r.unwrap_or(0)
}
