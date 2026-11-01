//! 终端核心（跨平台）。
//!
//! - 伪终端与 VT 状态机使用 `alacritty_terminal`：Windows 上是 ConPTY（exe 同目录或 PATH 中有
//!   微软的 `conpty.dll` + `OpenConsole.exe` 时优先使用，否则用系统自带的 ConPTY）；
//!   PTY 读取在它的后台线程中进行，状态保存在加锁的 `Term` 中
//! - [`Terminal::snapshot`] 在锁内把可见区域复制成与绘制无关的 [`Snapshot`]，UI 线程据此绘制
//! - [`keys`]：按键 → VT 序列；[`palette`]：Gruvbox Light 终端配色与 256 色换算
//!
//! 后台线程只通过构造时传入的回调通知 UI（UI 侧用 PostMessage 唤醒自己）。

pub mod keys;
pub mod palette;

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub use alacritty_terminal;
use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{self, Config, Term, TermMode};
use alacritty_terminal::tty;
use alacritty_terminal::vte::ansi::CursorShape;

/// 通知 UI 的事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TermEvent {
    /// 有新内容需要重绘
    Wakeup,
    Title(String),
    /// 程序请求写剪贴板（OSC 52）
    Clipboard(String),
    Bell,
    /// shell 退出（退出码）
    Exit(Option<i32>),
}

type Notify = Arc<dyn Fn(TermEvent) + Send + Sync>;

/// 交给 alacritty 的事件监听器。
#[derive(Clone)]
pub struct Listener {
    notify: Notify,
    /// 用于把终端的应答（设备属性查询等）写回 PTY
    sender: Arc<Mutex<Option<EventLoopSender>>>,
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        match event {
            Event::Wakeup => (self.notify)(TermEvent::Wakeup),
            Event::Title(t) => (self.notify)(TermEvent::Title(t)),
            Event::ResetTitle => (self.notify)(TermEvent::Title(String::new())),
            Event::ClipboardStore(_, s) => (self.notify)(TermEvent::Clipboard(s)),
            Event::Bell => (self.notify)(TermEvent::Bell),
            Event::ChildExit(status) => (self.notify)(TermEvent::Exit(status.code())),
            Event::Exit => (self.notify)(TermEvent::Exit(None)),
            Event::PtyWrite(text) => {
                if let Some(s) = self
                    .sender
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .as_ref()
                {
                    let _ = s.send(Msg::Input(Cow::Owned(text.into_bytes())));
                }
            }
            _ => {}
        }
    }
}

/// 终端尺寸（字符数）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub cols: usize,
    pub rows: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

fn window_size(size: Size, cell_w: u16, cell_h: u16) -> WindowSize {
    WindowSize {
        num_lines: size.rows.clamp(1, u16::MAX as usize) as u16,
        num_cols: size.cols.clamp(1, u16::MAX as usize) as u16,
        cell_width: cell_w.max(1),
        cell_height: cell_h.max(1),
    }
}

/// 一个格子（已换算好颜色）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub c: char,
    pub fg: u32,
    pub bg: u32,
    pub flags: u8,
}

impl Cell {
    pub const BOLD: u8 = 1;
    pub const ITALIC: u8 = 2;
    pub const UNDERLINE: u8 = 4;
    pub const STRIKE: u8 = 8;
    /// 双宽字符（占本格与下一格）
    pub const WIDE: u8 = 16;
    /// 双宽字符的第二格（不绘制）
    pub const SPACER: u8 = 32;
    pub const SELECTED: u8 = 64;
}

/// 光标形状。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cursor {
    Block,
    Underline,
    Beam,
    Hidden,
}

/// 可见区域的快照。
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub size: Size,
    /// 按行排列，`rows * cols` 个
    pub cells: Vec<Cell>,
    /// 光标位置（行, 列），不在可见区域时为 None
    pub cursor: Option<(usize, usize)>,
    pub cursor_shape: Cursor,
    /// 向上滚动了多少行（0 表示在最底部）
    pub display_offset: usize,
    pub mode: TermMode,
}

impl Snapshot {
    pub fn cell(&self, row: usize, col: usize) -> &Cell {
        &self.cells[row * self.size.cols + col]
    }

    /// 某一行的文字（去掉行尾空白，跳过双宽字符的占位格）。
    pub fn row_text(&self, row: usize) -> String {
        let mut s: String = (0..self.size.cols)
            .map(|c| self.cell(row, c))
            .filter(|c| c.flags & Cell::SPACER == 0)
            .map(|c| c.c)
            .collect();
        s.truncate(s.trim_end().len());
        s
    }
}

/// 一个终端实例。
pub struct Terminal {
    term: Arc<FairMutex<Term<Listener>>>,
    sender: Option<EventLoopSender>,
    size: Mutex<Size>,
}

/// 启动参数。
#[derive(Debug, Clone, Default)]
pub struct Options {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: HashMap<String, String>,
    pub size: Option<Size>,
    pub cell: (u16, u16),
}

fn new_term(size: Size, listener: Listener) -> Term<Listener> {
    let config = Config {
        scrolling_history: 10_000,
        ..Default::default()
    };
    Term::new(config, &size, listener)
}

impl Terminal {
    /// 启动 shell。`notify` 在后台线程中调用。
    pub fn spawn(
        opts: &Options,
        notify: Arc<dyn Fn(TermEvent) + Send + Sync>,
    ) -> std::io::Result<Terminal> {
        let size = opts.size.unwrap_or(Size { cols: 80, rows: 24 });
        let listener = Listener {
            notify,
            sender: Arc::new(Mutex::new(None)),
        };
        let term = Arc::new(FairMutex::new(new_term(size, listener.clone())));
        let pty_opts = tty::Options {
            shell: Some(tty::Shell::new(opts.program.clone(), opts.args.clone())),
            working_directory: opts.cwd.clone(),
            drain_on_exit: true,
            env: opts.env.clone(),
            ..Default::default()
        };
        let ws = window_size(size, opts.cell.0, opts.cell.1);
        let pty = tty::new(&pty_opts, ws, 0)?;
        let event_loop = EventLoop::new(term.clone(), listener.clone(), pty, true, false)?;
        let sender = event_loop.channel();
        *listener.sender.lock().unwrap_or_else(|e| e.into_inner()) = Some(sender.clone());
        let _reader = event_loop.spawn();
        Ok(Terminal {
            term,
            sender: Some(sender),
            size: Mutex::new(size),
        })
    }

    /// 不连接进程的终端（测试用）：用 [`feed`](Self::feed) 输入数据。
    pub fn detached(size: Size) -> Terminal {
        let listener = Listener {
            notify: Arc::new(|_| {}),
            sender: Arc::new(Mutex::new(None)),
        };
        Terminal {
            term: Arc::new(FairMutex::new(new_term(size, listener))),
            sender: None,
            size: Mutex::new(size),
        }
    }

    /// 把数据直接交给 VT 状态机（测试用）。
    pub fn feed(&self, bytes: &[u8]) {
        let mut p: alacritty_terminal::vte::ansi::Processor =
            alacritty_terminal::vte::ansi::Processor::new();
        let mut t = self.term.lock();
        p.advance(&mut *t, bytes);
    }

    /// 写入 PTY（键盘输入、粘贴）。
    pub fn write(&self, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        if let Some(s) = &self.sender {
            let _ = s.send(Msg::Input(Cow::Owned(bytes)));
        }
    }

    pub fn size(&self) -> Size {
        *self.size.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 改变尺寸（字符数与格子像素尺寸）。
    pub fn resize(&self, size: Size, cell_w: u16, cell_h: u16) {
        let size = Size {
            cols: size.cols.max(2),
            rows: size.rows.max(1),
        };
        {
            let mut cur = self.size.lock().unwrap_or_else(|e| e.into_inner());
            if *cur == size {
                return;
            }
            *cur = size;
        }
        self.term.lock().resize(size);
        if let Some(s) = &self.sender {
            let _ = s.send(Msg::Resize(window_size(size, cell_w, cell_h)));
        }
    }

    pub fn mode(&self) -> TermMode {
        *self.term.lock().mode()
    }

    /// 滚动回看（正数向上）。
    pub fn scroll(&self, lines: i32) {
        self.term.lock().scroll_display(Scroll::Delta(lines));
    }

    pub fn scroll_to_bottom(&self) {
        self.term.lock().scroll_display(Scroll::Bottom);
    }

    fn grid_point(t: &Term<Listener>, row: usize, col: usize) -> Point {
        let off = t.grid().display_offset();
        let cols = t.grid().columns();
        term::viewport_to_point(
            off,
            Point::new(row, Column(col.min(cols.saturating_sub(1)))),
        )
    }

    /// 开始选择（可见区域的行列；`word` 为按词选择，`line` 为整行）。
    pub fn select_start(&self, row: usize, col: usize, right_half: bool, kind: u8) {
        let mut t = self.term.lock();
        let p = Self::grid_point(&t, row, col);
        let side = if right_half { Side::Right } else { Side::Left };
        let ty = match kind {
            1 => SelectionType::Semantic,
            2 => SelectionType::Lines,
            _ => SelectionType::Simple,
        };
        t.selection = Some(Selection::new(ty, p, side));
    }

    pub fn select_update(&self, row: usize, col: usize, right_half: bool) {
        let mut t = self.term.lock();
        let p = Self::grid_point(&t, row, col);
        let side = if right_half { Side::Right } else { Side::Left };
        if let Some(s) = t.selection.as_mut() {
            s.update(p, side);
        }
    }

    pub fn clear_selection(&self) {
        self.term.lock().selection = None;
    }

    pub fn selection_text(&self) -> Option<String> {
        self.term
            .lock()
            .selection_to_string()
            .filter(|s| !s.is_empty())
    }

    /// 复制可见区域（在锁内完成，尽量短）。
    pub fn snapshot(&self) -> Snapshot {
        let t = self.term.lock();
        let content = t.renderable_content();
        let cols = t.grid().columns();
        let rows = t.grid().screen_lines();
        let off = content.display_offset;
        let colors = content.colors;
        let pal = palette::Palette::default();
        let mut cells = vec![
            Cell {
                c: ' ',
                fg: pal.fg,
                bg: pal.bg,
                flags: 0,
            };
            cols * rows
        ];
        let sel = content.selection;
        for ind in content.display_iter {
            let Some(vp) = term::point_to_viewport(off, ind.point) else {
                continue;
            };
            if vp.line >= rows || vp.column.0 >= cols {
                continue;
            }
            let cell = ind.cell;
            let mut fg = pal.resolve(cell.fg, colors, true);
            let mut bg = pal.resolve(cell.bg, colors, false);
            let f = cell.flags;
            if f.contains(Flags::DIM) {
                fg = palette::blend(fg, bg, 0.4);
            }
            if f.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            }
            if f.contains(Flags::HIDDEN) {
                fg = bg;
            }
            let mut flags = 0u8;
            if f.contains(Flags::BOLD) {
                flags |= Cell::BOLD;
            }
            if f.contains(Flags::ITALIC) {
                flags |= Cell::ITALIC;
            }
            if f.intersects(Flags::ALL_UNDERLINES) {
                flags |= Cell::UNDERLINE;
            }
            if f.contains(Flags::STRIKEOUT) {
                flags |= Cell::STRIKE;
            }
            if f.contains(Flags::WIDE_CHAR) {
                flags |= Cell::WIDE;
            }
            if f.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                flags |= Cell::SPACER;
            }
            if sel.as_ref().is_some_and(|s| s.contains(ind.point)) {
                flags |= Cell::SELECTED;
            }
            cells[vp.line * cols + vp.column.0] = Cell {
                c: cell.c,
                fg,
                bg,
                flags,
            };
        }
        let shape = match content.cursor.shape {
            CursorShape::Hidden => Cursor::Hidden,
            CursorShape::Underline => Cursor::Underline,
            CursorShape::Beam => Cursor::Beam,
            _ => Cursor::Block,
        };
        let cursor = term::point_to_viewport(off, content.cursor.point)
            .filter(|p| p.line < rows)
            .map(|p| (p.line, p.column.0.min(cols.saturating_sub(1))));
        Snapshot {
            size: Size { cols, rows },
            cells,
            cursor,
            cursor_shape: shape,
            display_offset: off,
            mode: content.mode,
        }
    }

    /// 结束 shell（关闭终端时调用）。
    pub fn shutdown(&self) {
        if let Some(s) = &self.sender {
            let _ = s.send(Msg::Shutdown);
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// 可见区域中的行号（0 起始）转为网格行（供外部需要时使用）。
pub fn viewport_line(display_offset: usize, row: usize) -> Line {
    Line(row as i32 - display_offset as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feed_text_colors_and_wide_chars() {
        let t = Terminal::detached(Size { cols: 20, rows: 4 });
        t.feed("ab\x1b[31mc\x1b[0m 中文\r\nline2".as_bytes());
        let s = t.snapshot();
        assert_eq!(s.row_text(0), "abc 中文");
        assert_eq!(s.row_text(1), "line2");
        let pal = palette::Palette::default();
        assert_eq!(s.cell(0, 0).fg, pal.fg);
        assert_eq!(s.cell(0, 2).fg, pal.ansi[1]);
        assert!(s.cell(0, 4).flags & Cell::WIDE != 0);
        assert!(s.cell(0, 5).flags & Cell::SPACER != 0);
        assert_eq!(s.cursor, Some((1, 5)));
    }

    #[test]
    fn scrollback_and_selection() {
        let t = Terminal::detached(Size { cols: 10, rows: 3 });
        for i in 0..10 {
            t.feed(format!("row{i}\r\n").as_bytes());
        }
        let s = t.snapshot();
        assert_eq!(s.row_text(0), "row8");
        t.scroll(3);
        let s = t.snapshot();
        assert_eq!(s.display_offset, 3);
        assert_eq!(s.row_text(0), "row5");
        t.select_start(0, 0, false, 0);
        t.select_update(1, 3, true);
        assert_eq!(t.selection_text().as_deref(), Some("row5\nrow6"));
        assert!(t.snapshot().cell(0, 1).flags & Cell::SELECTED != 0);
        t.scroll_to_bottom();
        assert_eq!(t.snapshot().display_offset, 0);
    }

    #[test]
    fn resize_keeps_content() {
        let t = Terminal::detached(Size { cols: 10, rows: 3 });
        t.feed(b"hello");
        t.resize(Size { cols: 40, rows: 10 }, 8, 16);
        let s = t.snapshot();
        assert_eq!(s.size, Size { cols: 40, rows: 10 });
        assert_eq!(s.row_text(0), "hello");
    }

    #[cfg(unix)]
    #[test]
    fn spawn_real_shell() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let exited = Arc::new(AtomicBool::new(false));
        let e2 = exited.clone();
        let t = Terminal::spawn(
            &Options {
                program: "sh".into(),
                args: vec!["-c".into(), "printf 'hello-from-pty'; sleep 0.2".into()],
                size: Some(Size { cols: 40, rows: 5 }),
                cell: (8, 16),
                ..Default::default()
            },
            Arc::new(move |e| {
                if matches!(e, TermEvent::Exit(_)) {
                    e2.store(true, Ordering::SeqCst);
                }
            }),
        )
        .unwrap();
        let start = std::time::Instant::now();
        while !t.snapshot().row_text(0).contains("hello-from-pty") {
            assert!(
                start.elapsed().as_secs() < 10,
                "{:?}",
                t.snapshot().row_text(0)
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        while !exited.load(Ordering::SeqCst) {
            assert!(start.elapsed().as_secs() < 10);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
}
