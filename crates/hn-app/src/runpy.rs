//! 运行 Python（`app` 的子模块）。
//!
//! - F5：用探测到的解释器运行当前文档（有路径且已修改时先保存；未命名文档写到临时文件），
//!   输出实时写进底部抽屉的"输出"面板；stderr 红色，ANSI 颜色照常显示
//! - Shift+F5：停止（结束整棵进程树）；Ctrl+F5：在新终端中运行（可交互）
//! - 子进程的读取线程把事件放进队列，再 `PostMessage(WM_APP_RUN)`；`pending` 合并重复唤醒，
//!   代号（gen）丢弃上一次运行的残留事件
//! - 输出面板双击 Traceback / 编译器错误行跳转到源码

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use hn_run::ansi::Decoder;
use hn_run::interp::{self, Interpreter};
use hn_run::proc::{self, Event, Spec};
use hn_sci::theme::out_style;

use super::*;
use drawer::Page;

pub(super) const WM_APP_RUN: u32 = WM_APP + 10;

/// 运行状态（UI 线程独占；队列与标志与后台线程共享）。
pub(super) struct RunState {
    gen: u32,
    running: Option<proc::Running>,
    started: Option<Instant>,
    stopped: bool,
    out: Decoder,
    err: Decoder,
    queue: Arc<Mutex<Vec<(u32, Event)>>>,
    pending: Arc<AtomicBool>,
    /// 输出面板最后一个字符是否为换行
    at_line_start: bool,
    /// 上一次运行的退出码（自测用）
    pub last_exit: Option<Option<i32>>,
}

impl Default for RunState {
    fn default() -> Self {
        RunState {
            gen: 0,
            running: None,
            started: None,
            stopped: false,
            out: Decoder::new(),
            err: Decoder::new(),
            queue: Arc::new(Mutex::new(Vec::new())),
            pending: Arc::new(AtomicBool::new(false)),
            at_line_start: true,
            last_exit: None,
        }
    }
}

/// PowerShell 单引号字符串。
fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// cmd 双引号字符串。
fn cmd_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('"', ""))
}

impl App {
    /// 准备要运行的脚本：返回脚本路径。
    fn prepare_script(&self) -> Option<PathBuf> {
        if self.active_doc(|d| d.is_large).unwrap_or(true) {
            self.show_message("大文件视图中的文档不能运行");
            return None;
        }
        let path = self.active_doc(|d| d.path.clone()).flatten();
        match path {
            Some(p) => {
                if self.is_dirty(self.active.get()) && !self.save_active() {
                    return None;
                }
                Some(p)
            }
            None => {
                let sci = self.sci()?;
                let n = self.active_doc(|d| d.untitled).unwrap_or(0);
                let p = std::env::temp_dir()
                    .join("HurshNote")
                    .join("run")
                    .join(format!("untitled-{n}.py"));
                // 临时文件属于程序自己，不是用户文件
                if let Err(e) = hn_core::atomic::write_atomic(&p, &sci.bytes()) {
                    self.show_message(&format!("无法写入临时文件：{e}"));
                    return None;
                }
                Some(p)
            }
        }
    }

    fn find_interpreter(&self, script: &Path) -> Option<Interpreter> {
        let configured = {
            let s = self.settings.borrow();
            let p = s.python.trim().to_string();
            (!p.is_empty()).then(|| PathBuf::from(p))
        };
        let mut dirs: Vec<PathBuf> = script.parent().map(Path::to_path_buf).into_iter().collect();
        dirs.extend(self.settings.borrow().workspace.iter().cloned());
        interp::find(configured.as_deref(), &dirs)
    }

    /// F5 / Ctrl+F5。
    pub(super) fn run_python(&self, in_terminal: bool) {
        let Some(script) = self.prepare_script() else {
            return;
        };
        let dir = script
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(std::env::temp_dir);
        let Some(interp) = self.find_interpreter(&script) else {
            self.output_clear();
            self.output_append(
                "找不到 Python 解释器。\n请安装 Python（python.org 或 winget install Python.Python.3），\
                 或在设置文件 settings.toml 中用 python = \"C:\\\\路径\\\\python.exe\" 指定。\n",
                out_style::ERROR,
            );
            self.show_page(Page::Output, false);
            return;
        };
        if in_terminal {
            self.run_in_terminal(&interp, &script, &dir);
            return;
        }
        self.stop_run();
        let gen = {
            let mut r = self.run.borrow_mut();
            r.gen = r.gen.wrapping_add(1);
            r.out = Decoder::new();
            r.err = Decoder::new();
            r.stopped = false;
            r.last_exit = None;
            r.queue.lock().unwrap_or_else(|e| e.into_inner()).clear();
            r.gen
        };
        *self.run_dir.borrow_mut() = Some(dir.clone());
        self.output_clear();
        self.run.borrow_mut().at_line_start = true;
        let mut args = interp.args.clone();
        args.push("-u".into());
        args.push(script.display().to_string());
        self.output_line(
            &format!("▶ {} {}", interp.program.display(), args.join(" ")),
            out_style::INFO,
        );
        self.show_page(Page::Output, false);
        let spec = Spec {
            program: interp.program.clone(),
            args,
            cwd: Some(dir),
            env: vec![
                ("PYTHONUTF8".into(), "1".into()),
                ("PYTHONIOENCODING".into(), "utf-8".into()),
            ],
        };
        let (queue, pending) = {
            let r = self.run.borrow();
            (r.queue.clone(), r.pending.clone())
        };
        let hwnd = self.hwnd() as isize;
        let sink: proc::Sink = Arc::new(move |e: Event| {
            queue
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push((gen, e));
            if !pending.swap(true, Ordering::SeqCst) {
                unsafe { PostMessageW(hwnd as HWND, WM_APP_RUN, 0, 0) };
            }
        });
        match proc::spawn(&spec, sink) {
            Ok(r) => {
                let mut st = self.run.borrow_mut();
                st.running = Some(r);
                st.started = Some(Instant::now());
                drop(st);
                self.set_drawer_info("正在运行…（Shift+F5 停止）");
            }
            Err(e) => {
                self.output_line(&format!("无法启动解释器：{e}"), out_style::ERROR);
                self.set_drawer_info("");
            }
        }
    }

    /// Shift+F5：停止正在运行的程序（退出事件到达后显示结果）。
    pub(super) fn stop_run(&self) {
        let mut r = self.run.borrow_mut();
        if let Some(p) = &r.running {
            p.stop();
            r.stopped = true;
        }
    }

    pub(super) fn is_running(&self) -> bool {
        self.run.borrow().running.is_some()
    }

    fn run_in_terminal(&self, interp: &Interpreter, script: &Path, dir: &Path) {
        let Some(id) = self.new_terminal_in(None, Some(dir.to_path_buf())) else {
            return;
        };
        let kind = crate::term::view(id).map(|v| v.kind).unwrap_or("other");
        let prog = interp.program.display().to_string();
        let script = script.display().to_string();
        let line = match kind {
            "powershell" | "pwsh" => {
                let mut v = vec!["&".to_string(), ps_quote(&prog)];
                v.extend(interp.args.iter().map(|a| ps_quote(a)));
                v.push(ps_quote(&script));
                v.join(" ")
            }
            _ => {
                let mut v = vec![cmd_quote(&prog)];
                v.extend(interp.args.iter().cloned());
                v.push(cmd_quote(&script));
                v.join(" ")
            }
        };
        crate::term::write(id, format!("{line}\r").into_bytes());
    }

    /// 处理后台送来的输出。
    pub(super) fn on_run_msgs(&self) {
        let (events, gen) = {
            let r = self.run.borrow();
            r.pending.store(false, Ordering::SeqCst);
            let ev = std::mem::take(&mut *r.queue.lock().unwrap_or_else(|e| e.into_inner()));
            (ev, r.gen)
        };
        for (g, ev) in events {
            if g != gen {
                continue;
            }
            match ev {
                Event::Stdout(b) => {
                    let segs = self.run.borrow_mut().out.feed(&b);
                    for s in segs {
                        let style = s.color.map_or(out_style::NORMAL, |c| out_style::ANSI + c);
                        self.output_text(&s.text, style);
                    }
                }
                Event::Stderr(b) => {
                    let segs = self.run.borrow_mut().err.feed(&b);
                    for s in segs {
                        let style = s.color.map_or(out_style::ERROR, |c| out_style::ANSI + c);
                        self.output_text(&s.text, style);
                    }
                }
                Event::Exit(code) => {
                    let (rest_out, rest_err, secs, stopped) = {
                        let mut r = self.run.borrow_mut();
                        r.running = None;
                        r.last_exit = Some(code);
                        (
                            r.out.finish(),
                            r.err.finish(),
                            r.started.map(|t| t.elapsed().as_secs_f64()).unwrap_or(0.0),
                            r.stopped,
                        )
                    };
                    for s in rest_out {
                        self.output_text(&s.text, out_style::NORMAL);
                    }
                    for s in rest_err {
                        self.output_text(&s.text, out_style::ERROR);
                    }
                    let what = if stopped {
                        "已停止".to_string()
                    } else {
                        match code {
                            Some(c) => format!("进程已退出，代码 {c}"),
                            None => "进程已结束".to_string(),
                        }
                    };
                    let style = if code == Some(0) && !stopped {
                        out_style::OK
                    } else {
                        out_style::INFO
                    };
                    self.output_line(&format!("[{what}，用时 {secs:.2} 秒]"), style);
                    self.set_drawer_info(&what);
                }
            }
        }
    }

    /// 追加文字并记录是否停在行首。
    fn output_text(&self, text: &str, style: u8) {
        if text.is_empty() {
            return;
        }
        self.output_append(text, style);
        self.run.borrow_mut().at_line_start = text.ends_with(['\n', '\r']);
    }

    /// 在新的一行写一行文字。
    fn output_line(&self, text: &str, style: u8) {
        if !self.run.borrow().at_line_start {
            self.output_text("\n", out_style::NORMAL);
        }
        self.output_text(&format!("{text}\n"), style);
    }

    // ---- 自测 ----

    /// 运行脚本并等待结束（自测：在当前线程处理消息队列）。
    pub(crate) fn st_run_python(&self, script: &Path, timeout_ms: u64) -> Option<Option<i32>> {
        self.open_path(script, None, false);
        self.run_python(false);
        let start = Instant::now();
        loop {
            self.on_run_msgs();
            if let Some(c) = self.run.borrow().last_exit {
                return Some(c);
            }
            if !self.is_running() && self.run.borrow().last_exit.is_none() {
                // 没有启动（找不到解释器等）
                return None;
            }
            if start.elapsed().as_millis() as u64 > timeout_ms {
                self.stop_run();
                return None;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    pub(crate) fn st_python_available(&self, script: &Path) -> bool {
        self.find_interpreter(script).is_some()
    }

    /// 输出面板中双击第 `line` 行（0 起始）的效果。
    pub(crate) fn st_output_jump(&self, needle: &str) -> bool {
        let Some(o) = self.output.get() else {
            return false;
        };
        for l in 0..o.line_count() {
            let text = o.line_text(l);
            if text.contains(needle) {
                let base = self.run_dir.borrow().clone();
                if let Some(loc) = hn_run::parse::location(&text, base.as_deref()) {
                    self.open_path(&loc.path, Some((loc.line, None)), false);
                    return true;
                }
            }
        }
        false
    }
}
