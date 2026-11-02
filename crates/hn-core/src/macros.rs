//! 宏：录制下来的步骤序列及其持久化（与平台无关，可在 Linux 上测试）。
//!
//! 步骤来源有三种：
//! - Scintilla 的 `SCN_MACRORECORD` 通知（按键、光标移动、输入的文字……）
//! - 菜单命令（行操作、大小写、书签……）
//! - 查找对话框的操作（查找下一个、替换、全部替换……）
//!
//! 坑：Scintilla 把一次回车录成逐个换行字符的 `SCI_REPLACESEL`（CRLF 模式下是 `"\r"`、`"\n"` 两条），
//! 并且是在发出 `SCN_CHARADDED` **之后**才录。回放 `REPLACESEL` 不会触发 `SCN_CHARADDED`，
//! 自动缩进与列表续写就不会发生，回放结果与录制时不同。所以这里把它们合并为一个 [`Step::NewLine`]，
//! 回放时发 `SCI_NEWLINE`，让编辑器像真的按了回车一样处理。

use serde::{Deserialize, Serialize};

/// 宏文件格式版本。
pub const VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Step {
    /// 一条 Scintilla 消息；`text` 为字符串参数（REPLACESEL、SEARCHNEXT 等）
    Sci {
        msg: u32,
        #[serde(default)]
        wp: u64,
        #[serde(default)]
        lp: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },
    /// 一次回车
    NewLine,
    /// 菜单命令
    Command { id: u16 },
    /// 查找对话框的操作
    Find(FindStep),
}

/// 查找对话框的一次操作（与 `hn-app` 的查找状态一一对应）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct FindStep {
    /// next / prev / count / mark_all / clear_marks / replace / replace_all
    pub action: String,
    pub pattern: String,
    pub replace: String,
    /// 0 普通，1 扩展（转义），2 正则
    pub mode: u8,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub dot_all: bool,
    pub wrap: bool,
    pub in_selection: bool,
}

/// 录制器。
#[derive(Debug, Default)]
pub struct Recorder {
    steps: Vec<Step>,
    /// 刚录了一个 "\r"（CRLF 模式下紧随的 "\n" 属于同一次回车）
    pending_cr: bool,
}

impl Recorder {
    pub fn new() -> Recorder {
        Recorder::default()
    }

    /// 记录一条 Scintilla 宏通知。`replace_sel` 为 `SCI_REPLACESEL` 的消息号。
    /// 连续输入的文字合并为一步。
    pub fn sci(&mut self, msg: u32, wp: u64, lp: i64, text: Option<String>, replace_sel: u32) {
        if msg == replace_sel {
            if let Some(t) = text.as_deref() {
                match t {
                    "\r" => {
                        self.steps.push(Step::NewLine);
                        self.pending_cr = true;
                        return;
                    }
                    "\n" => {
                        if !std::mem::take(&mut self.pending_cr) {
                            self.steps.push(Step::NewLine);
                        }
                        return;
                    }
                    _ => {}
                }
                self.pending_cr = false;
                if let Some(Step::Sci {
                    msg: m,
                    text: Some(prev),
                    ..
                }) = self.steps.last_mut()
                {
                    if *m == replace_sel
                        && !t.contains(['\r', '\n'])
                        && !prev.contains(['\r', '\n'])
                    {
                        prev.push_str(t);
                        return;
                    }
                }
            }
        }
        self.pending_cr = false;
        self.steps.push(Step::Sci { msg, wp, lp, text });
    }

    pub fn command(&mut self, id: u16) {
        self.pending_cr = false;
        self.steps.push(Step::Command { id });
    }

    pub fn find(&mut self, f: FindStep) {
        self.pending_cr = false;
        self.steps.push(Step::Find(f));
    }

    pub fn len(&self) -> usize {
        self.steps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    pub fn finish(self) -> Vec<Step> {
        self.steps
    }
}

#[derive(Serialize, Deserialize)]
struct File {
    version: u32,
    steps: Vec<Step>,
}

/// 序列化为 JSON。
pub fn to_json(steps: &[Step]) -> String {
    serde_json::to_string_pretty(&File {
        version: VERSION,
        steps: steps.to_vec(),
    })
    .unwrap_or_default()
}

/// 从 JSON 读取。版本更新的文件拒绝读取（可能含有不认识的步骤）。
pub fn from_json(s: &str) -> Result<Vec<Step>, String> {
    let f: File = serde_json::from_str(s).map_err(|e| e.to_string())?;
    if f.version > VERSION {
        return Err(format!("宏文件版本 {} 比程序新", f.version));
    }
    Ok(f.steps)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RS: u32 = 2170;

    fn rs(t: &str) -> (u32, u64, i64, Option<String>) {
        (RS, 0, 0, Some(t.to_string()))
    }

    fn feed(r: &mut Recorder, items: &[(u32, u64, i64, Option<String>)]) {
        for (m, w, l, t) in items {
            r.sci(*m, *w, *l, t.clone(), RS);
        }
    }

    #[test]
    fn crlf_newline_becomes_one_step() {
        let mut r = Recorder::new();
        feed(&mut r, &[rs("a"), rs("\r"), rs("\n"), rs("b")]);
        assert_eq!(
            r.finish(),
            vec![
                Step::Sci {
                    msg: RS,
                    wp: 0,
                    lp: 0,
                    text: Some("a".into())
                },
                Step::NewLine,
                Step::Sci {
                    msg: RS,
                    wp: 0,
                    lp: 0,
                    text: Some("b".into())
                },
            ]
        );
    }

    #[test]
    fn lf_and_cr_modes() {
        let mut r = Recorder::new();
        feed(&mut r, &[rs("\n"), rs("\n")]);
        assert_eq!(r.finish(), vec![Step::NewLine, Step::NewLine]);
        let mut r = Recorder::new();
        feed(&mut r, &[rs("\r"), rs("\r")]);
        assert_eq!(r.finish(), vec![Step::NewLine, Step::NewLine]);
    }

    #[test]
    fn typed_text_merges_until_other_step() {
        let mut r = Recorder::new();
        feed(&mut r, &[rs("你"), rs("好"), (2302, 0, 0, None), rs("x")]);
        r.command(1211);
        feed(&mut r, &[rs("y")]);
        let s = r.finish();
        assert_eq!(s.len(), 5);
        assert_eq!(
            s[0],
            Step::Sci {
                msg: RS,
                wp: 0,
                lp: 0,
                text: Some("你好".into())
            }
        );
        assert_eq!(s[3], Step::Command { id: 1211 });
    }

    #[test]
    fn json_roundtrip_and_version_check() {
        let mut r = Recorder::new();
        feed(&mut r, &[rs("a\"b"), (2300, 3, -1, None)]);
        r.find(FindStep {
            action: "replace_all".into(),
            pattern: "a".into(),
            replace: "b".into(),
            mode: 2,
            ..Default::default()
        });
        let steps = r.finish();
        let j = to_json(&steps);
        assert_eq!(from_json(&j).unwrap(), steps);
        assert!(from_json(&j.replace("\"version\": 1", "\"version\": 99")).is_err());
        assert!(from_json("not json").is_err());
    }
}
