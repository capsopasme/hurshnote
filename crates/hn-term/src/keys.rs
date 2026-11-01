//! 按键 → 发给终端程序的字节序列（xterm 约定）。
//!
//! 普通字符由窗口的 `WM_CHAR` 直接以 UTF-8 写入（Ctrl+字母已被系统转换成控制字符），
//! 这里只处理方向键、功能键等不产生字符的按键。

/// 不产生字符的按键。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    Backspace,
    Tab,
    Enter,
    Escape,
    /// F1–F12
    F(u8),
}

/// 修饰键。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Mods {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

impl Mods {
    /// xterm 的修饰参数：1 + Shift + 2×Alt + 4×Ctrl；无修饰时为 None。
    fn param(self) -> Option<u8> {
        let m = u8::from(self.shift) + 2 * u8::from(self.alt) + 4 * u8::from(self.ctrl);
        (m > 0).then_some(m + 1)
    }
}

/// 按键对应的字节序列。`app_cursor` 为应用光标模式（DECCKM）。
pub fn bytes(key: Key, mods: Mods, app_cursor: bool) -> Option<Vec<u8>> {
    let m = mods.param();
    let csi_letter = |c: char| -> Vec<u8> {
        match m {
            Some(m) => format!("\x1b[1;{m}{c}").into_bytes(),
            None if app_cursor => format!("\x1bO{c}").into_bytes(),
            None => format!("\x1b[{c}").into_bytes(),
        }
    };
    let tilde = |n: u8| -> Vec<u8> {
        match m {
            Some(m) => format!("\x1b[{n};{m}~").into_bytes(),
            None => format!("\x1b[{n}~").into_bytes(),
        }
    };
    let alt_prefix = |b: &[u8]| -> Vec<u8> {
        let mut v = Vec::with_capacity(b.len() + 1);
        if mods.alt {
            v.push(0x1b);
        }
        v.extend_from_slice(b);
        v
    };
    Some(match key {
        Key::Up => csi_letter('A'),
        Key::Down => csi_letter('B'),
        Key::Right => csi_letter('C'),
        Key::Left => csi_letter('D'),
        Key::Home => csi_letter('H'),
        Key::End => csi_letter('F'),
        Key::Insert => tilde(2),
        Key::Delete => tilde(3),
        Key::PageUp => tilde(5),
        Key::PageDown => tilde(6),
        Key::Backspace => alt_prefix(if mods.ctrl { b"\x08" } else { b"\x7f" }),
        Key::Tab if mods.shift => b"\x1b[Z".to_vec(),
        Key::Tab => alt_prefix(b"\t"),
        Key::Enter => alt_prefix(b"\r"),
        Key::Escape => b"\x1b".to_vec(),
        Key::F(n @ 1..=4) => {
            let c = (b'P' + n - 1) as char;
            match m {
                Some(m) => format!("\x1b[1;{m}{c}").into_bytes(),
                None => format!("\x1bO{c}").into_bytes(),
            }
        }
        Key::F(n @ 5..=12) => {
            const CODES: [u8; 8] = [15, 17, 18, 19, 20, 21, 23, 24];
            tilde(CODES[(n - 5) as usize])
        }
        Key::F(_) => return None,
    })
}

/// 粘贴的文字：统一换行为 CR；括号粘贴模式下加上起止标记。
pub fn paste(text: &str, bracketed: bool) -> Vec<u8> {
    let body = text.replace("\r\n", "\r").replace('\n', "\r");
    if bracketed {
        // 去掉文字中可能夹带的结束标记，防止注入
        let body = body.replace("\x1b[201~", "");
        format!("\x1b[200~{body}\x1b[201~").into_bytes()
    } else {
        body.into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(k: Key, m: Mods, app: bool) -> String {
        String::from_utf8(bytes(k, m, app).unwrap()).unwrap()
    }

    #[test]
    fn arrows_and_modes() {
        let none = Mods::default();
        assert_eq!(s(Key::Up, none, false), "\x1b[A");
        assert_eq!(s(Key::Up, none, true), "\x1bOA");
        let ctrl = Mods {
            ctrl: true,
            ..Default::default()
        };
        assert_eq!(s(Key::Right, ctrl, true), "\x1b[1;5C");
        let shift = Mods {
            shift: true,
            ..Default::default()
        };
        assert_eq!(s(Key::Home, shift, false), "\x1b[1;2H");
        assert_eq!(s(Key::Tab, shift, false), "\x1b[Z");
    }

    #[test]
    fn editing_and_function_keys() {
        let none = Mods::default();
        assert_eq!(s(Key::Delete, none, false), "\x1b[3~");
        assert_eq!(s(Key::PageDown, none, false), "\x1b[6~");
        assert_eq!(s(Key::Backspace, none, false), "\x7f");
        assert_eq!(s(Key::F(1), none, false), "\x1bOP");
        assert_eq!(s(Key::F(5), none, false), "\x1b[15~");
        assert_eq!(s(Key::F(12), none, false), "\x1b[24~");
        let alt = Mods {
            alt: true,
            ..Default::default()
        };
        assert_eq!(s(Key::Enter, alt, false), "\x1b\r");
        assert_eq!(s(Key::Delete, alt, false), "\x1b[3;3~");
        assert!(bytes(Key::F(13), none, false).is_none());
    }

    #[test]
    fn paste_modes() {
        assert_eq!(paste("a\r\nb\nc", false), b"a\rb\rc");
        assert_eq!(paste("x", true), b"\x1b[200~x\x1b[201~");
        assert_eq!(paste("x\x1b[201~y", true), b"\x1b[200~xy\x1b[201~");
    }
}
