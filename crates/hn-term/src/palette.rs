//! 终端配色（Gruvbox Light）与颜色换算。

use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, Rgb};

/// 0xRRGGBB。
fn rgb(c: Rgb) -> u32 {
    ((c.r as u32) << 16) | ((c.g as u32) << 8) | c.b as u32
}

/// 按比例 `t`（0..1）把颜色 `a` 混向 `b`。
pub fn blend(a: u32, b: u32, t: f32) -> u32 {
    let ch = |c: u32, s: u32| -> u32 {
        let x = ((c >> s) & 0xff) as f32;
        let y = ((b >> s) & 0xff) as f32;
        ((x + (y - x) * t).round().clamp(0.0, 255.0) as u32) << s
    };
    ch(a, 16) | ch(a, 8) | ch(a, 0)
}

/// 终端调色板。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub fg: u32,
    pub bg: u32,
    pub cursor: u32,
    pub selection: u32,
    pub ansi: [u32; 16],
}

impl Default for Palette {
    /// Gruvbox Light 的终端配色。
    fn default() -> Self {
        Palette {
            fg: 0x3c3836,
            bg: 0xfbf1c7,
            cursor: 0x3c3836,
            selection: 0xd5c4a1,
            ansi: [
                0xfbf1c7, 0xcc241d, 0x98971a, 0xd79921, 0x458588, 0xb16286, 0x689d6a, 0x7c6f64,
                0x928374, 0x9d0006, 0x79740e, 0xb57614, 0x076678, 0x8f3f71, 0x427b58, 0x3c3836,
            ],
        }
    }
}

impl Palette {
    /// 256 色中的第 `i` 个。
    pub fn indexed(&self, i: u8) -> u32 {
        match i {
            0..=15 => self.ansi[i as usize],
            16..=231 => {
                const LEVELS: [u32; 6] = [0, 95, 135, 175, 215, 255];
                let n = (i - 16) as usize;
                (LEVELS[n / 36] << 16) | (LEVELS[(n / 6) % 6] << 8) | LEVELS[n % 6]
            }
            _ => {
                let v = 8 + 10 * (i as u32 - 232);
                (v << 16) | (v << 8) | v
            }
        }
    }

    fn named(&self, i: usize, is_fg: bool) -> u32 {
        match i {
            0..=15 => self.ansi[i],
            256 => self.fg,
            257 => self.bg,
            258 => self.cursor,
            259..=266 => blend(self.ansi[i - 259], self.bg, 0.35),
            267 => self.fg,
            268 => blend(self.fg, self.bg, 0.35),
            _ if is_fg => self.fg,
            _ => self.bg,
        }
    }

    /// 把终端颜色换算为 0xRRGGBB（程序用 OSC 4/10/11 改过的颜色优先）。
    pub fn resolve(&self, c: Color, overrides: &Colors, is_fg: bool) -> u32 {
        match c {
            Color::Spec(v) => rgb(v),
            Color::Named(n) => {
                let i = n as usize;
                match overrides[i] {
                    Some(v) => rgb(v),
                    None => self.named(i, is_fg),
                }
            }
            Color::Indexed(i) => match overrides[i as usize] {
                Some(v) => rgb(v),
                None => self.indexed(i),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cube_and_gray() {
        let p = Palette::default();
        assert_eq!(p.indexed(16), 0x000000);
        assert_eq!(p.indexed(231), 0xffffff);
        assert_eq!(p.indexed(196), 0xff0000);
        assert_eq!(p.indexed(232), 0x080808);
        assert_eq!(p.indexed(255), 0xeeeeee);
        assert_eq!(p.indexed(1), p.ansi[1]);
    }

    #[test]
    fn blend_halfway() {
        assert_eq!(blend(0x000000, 0xffffff, 0.5), 0x808080);
        assert_eq!(blend(0x102030, 0x102030, 0.7), 0x102030);
    }
}
