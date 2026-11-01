//! 输出流解码：UTF-8（跨块拼接不完整的字符）+ ANSI SGR 颜色。
//!
//! 只解释 SGR（`ESC [ ... m`）中的前景色：30–37、90–97、39、0；其他 CSI/OSC 序列丢弃。
//! `\r\n` 规范化为 `\n`，单独的 `\r` 保留给调用方（输出面板按换行处理）。

/// 一段同色文字。`color` 为 0–15 的调色板下标，None 表示默认色。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub text: String,
    pub color: Option<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Text,
    Esc,
    Csi,
    Osc,
    OscEsc,
}

/// 增量解码器（每个输出流一个）。
#[derive(Debug, Clone)]
pub struct Decoder {
    utf8: Vec<u8>,
    state: State,
    params: String,
    color: Option<u8>,
    pending_cr: bool,
}

impl Default for Decoder {
    fn default() -> Self {
        Decoder {
            utf8: Vec::new(),
            state: State::Text,
            params: String::new(),
            color: None,
            pending_cr: false,
        }
    }
}

impl Decoder {
    pub fn new() -> Decoder {
        Decoder::default()
    }

    fn apply_sgr(&mut self) {
        let params = std::mem::take(&mut self.params);
        if params.is_empty() {
            self.color = None;
            return;
        }
        let nums: Vec<u32> = params.split(';').map(|p| p.parse().unwrap_or(0)).collect();
        let mut i = 0;
        while i < nums.len() {
            match nums[i] {
                0 | 39 => self.color = None,
                n @ 30..=37 => self.color = Some((n - 30) as u8),
                n @ 90..=97 => self.color = Some((n - 90 + 8) as u8),
                // 38;5;n / 38;2;r;g;b：取最接近的基础色太复杂，按默认色处理并跳过参数
                38 => {
                    if nums.get(i + 1) == Some(&5) {
                        if let Some(&n) = nums.get(i + 2) {
                            self.color = (n < 16).then_some(n as u8);
                        }
                        i += 2;
                    } else if nums.get(i + 1) == Some(&2) {
                        self.color = None;
                        i += 4;
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }

    /// 解码一块输出，返回分好色的文字段。
    pub fn feed(&mut self, data: &[u8]) -> Vec<Segment> {
        let mut bytes = std::mem::take(&mut self.utf8);
        bytes.extend_from_slice(data);
        // 末尾不完整的 UTF-8 字符留到下一块
        let keep = incomplete_tail(&bytes);
        let tail = bytes.split_off(bytes.len() - keep);
        self.utf8 = tail;
        let text = String::from_utf8_lossy(&bytes);

        let mut out: Vec<Segment> = Vec::new();
        let mut cur = String::new();
        let flush = |cur: &mut String, color: Option<u8>, out: &mut Vec<Segment>| {
            if cur.is_empty() {
                return;
            }
            match out.last_mut() {
                Some(last) if last.color == color => last.text.push_str(cur),
                _ => out.push(Segment {
                    text: std::mem::take(cur),
                    color,
                }),
            }
            cur.clear();
        };
        for c in text.chars() {
            match self.state {
                State::Text => match c {
                    '\u{1b}' => {
                        flush(&mut cur, self.color, &mut out);
                        self.state = State::Esc;
                    }
                    '\r' => {
                        if self.pending_cr {
                            cur.push('\r');
                        }
                        self.pending_cr = true;
                    }
                    '\n' => {
                        self.pending_cr = false;
                        cur.push('\n');
                    }
                    _ => {
                        if self.pending_cr {
                            cur.push('\r');
                            self.pending_cr = false;
                        }
                        if c != '\u{7}' && c != '\u{8}' {
                            cur.push(c);
                        }
                    }
                },
                State::Esc => {
                    self.state = match c {
                        '[' => {
                            self.params.clear();
                            State::Csi
                        }
                        ']' => State::Osc,
                        _ => State::Text,
                    }
                }
                State::Csi => {
                    if c.is_ascii_digit() || c == ';' {
                        self.params.push(c);
                    } else if ('\u{40}'..='\u{7e}').contains(&c) {
                        if c == 'm' {
                            self.apply_sgr();
                        }
                        self.params.clear();
                        self.state = State::Text;
                    }
                    // 其他中间字符（? 等）忽略
                }
                State::Osc => match c {
                    '\u{7}' => self.state = State::Text,
                    '\u{1b}' => self.state = State::OscEsc,
                    _ => {}
                },
                State::OscEsc => self.state = State::Text,
            }
        }
        flush(&mut cur, self.color, &mut out);
        out
    }

    /// 流结束：输出残留的半个字符与 CR。
    pub fn finish(&mut self) -> Vec<Segment> {
        let mut s = String::from_utf8_lossy(&std::mem::take(&mut self.utf8)).into_owned();
        if std::mem::take(&mut self.pending_cr) {
            s.insert(0, '\r');
        }
        if s.is_empty() {
            Vec::new()
        } else {
            vec![Segment {
                text: s,
                color: self.color,
            }]
        }
    }
}

/// `bytes` 末尾不完整的 UTF-8 序列长度。
fn incomplete_tail(bytes: &[u8]) -> usize {
    let n = bytes.len();
    for back in 1..=3.min(n) {
        let b = bytes[n - back];
        if b & 0xC0 == 0x80 {
            continue; // 续字节
        }
        let need = match b {
            0xC0..=0xDF => 2,
            0xE0..=0xEF => 3,
            0xF0..=0xF7 => 4,
            _ => return 0,
        };
        return if need > back { back } else { 0 };
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(segs: &[Segment]) -> String {
        segs.iter().map(|s| s.text.as_str()).collect()
    }

    #[test]
    fn colors_and_reset() {
        let mut d = Decoder::new();
        let s = d.feed(b"ok \x1b[31mred\x1b[0m done\x1b[92m!\x1b[m");
        assert_eq!(
            s,
            vec![
                Segment {
                    text: "ok ".into(),
                    color: None
                },
                Segment {
                    text: "red".into(),
                    color: Some(1)
                },
                Segment {
                    text: " done".into(),
                    color: None
                },
                Segment {
                    text: "!".into(),
                    color: Some(10)
                },
            ]
        );
    }

    #[test]
    fn split_utf8_and_escape_across_chunks() {
        let mut d = Decoder::new();
        let bytes = "中文\x1b[33m黄".as_bytes();
        let mut all = Vec::new();
        for chunk in bytes.chunks(2) {
            all.extend(d.feed(chunk));
        }
        assert_eq!(plain(&all), "中文黄");
        assert_eq!(all.last().unwrap().color, Some(3));
    }

    #[test]
    fn crlf_and_other_sequences() {
        let mut d = Decoder::new();
        let s = d.feed(b"a\r\nb\x1b[2K\x1b]0;title\x07c\r");
        assert_eq!(plain(&s), "a\nbc");
        let s = d.feed(b"\n");
        assert_eq!(plain(&s), "\n");
        let s = d.feed(b"x\ry");
        assert_eq!(plain(&s), "x\ry");
    }

    #[test]
    fn incomplete() {
        assert_eq!(incomplete_tail("中".as_bytes()), 0);
        assert_eq!(incomplete_tail(&"中".as_bytes()[..2]), 2);
        assert_eq!(incomplete_tail(&"😀".as_bytes()[..3]), 3);
        assert_eq!(incomplete_tail(b"abc"), 0);
    }
}
