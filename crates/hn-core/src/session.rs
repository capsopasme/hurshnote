//! 会话清单：打开了哪些文档、各自的状态与快照。保存为 JSON。
//!
//! 字段都有默认值，旧版本写出的清单可以被新版本读取。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::stamp::DiskStamp;

pub const VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    pub version: u32,
    /// 当前文档在 `docs` 中的下标
    pub active: usize,
    pub docs: Vec<SessionDoc>,
}

impl Default for Session {
    fn default() -> Self {
        Session {
            version: VERSION,
            active: 0,
            docs: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SessionDoc {
    /// None 表示未命名文档
    pub path: Option<PathBuf>,
    /// 未命名文档的序号
    pub untitled: u32,
    /// 快照文件名（有未保存修改时存在）
    pub snapshot: Option<String>,
    /// 编码标识（见 `hn_text::encoding::TextEncoding::id`）
    pub encoding: String,
    pub bom: bool,
    /// "crlf" / "lf" / "cr"
    pub eol: String,
    /// 语言 id
    pub lang: String,
    pub read_only: bool,
    pub encrypted: bool,
    /// 编码等元数据有未保存的修改
    pub meta_dirty: bool,
    pub anchor: usize,
    pub caret: usize,
    pub first_line: usize,
    pub bookmarks: Vec<usize>,
    /// 折叠起来的折叠头所在行
    pub folds: Vec<usize>,
    /// 载入/保存时的磁盘状态，用于检测关闭期间文件被外部修改
    pub disk: Option<DiskStamp>,
}

impl Session {
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("会话序列化不会失败")
    }

    pub fn from_json(text: &str) -> Result<Session, serde_json::Error> {
        let mut s: Session = serde_json::from_str(text)?;
        if s.active >= s.docs.len() {
            s.active = 0;
        }
        Ok(s)
    }

    /// 被引用的快照文件名。
    pub fn snapshot_names(&self) -> std::collections::HashSet<String> {
        self.docs
            .iter()
            .filter_map(|d| d.snapshot.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let s = Session {
            version: VERSION,
            active: 1,
            docs: vec![
                SessionDoc {
                    path: Some(PathBuf::from(r"C:\文档\a.txt")),
                    encoding: "GBK".into(),
                    eol: "crlf".into(),
                    lang: "text".into(),
                    bookmarks: vec![1, 5],
                    disk: Some(DiskStamp {
                        mtime: 1,
                        size: 2,
                        hash: 3,
                    }),
                    ..Default::default()
                },
                SessionDoc {
                    untitled: 2,
                    snapshot: Some("ab.snap".into()),
                    ..Default::default()
                },
            ],
        };
        let back = Session::from_json(&s.to_json()).unwrap();
        assert_eq!(back, s);
        assert_eq!(back.snapshot_names().len(), 1);
    }

    #[test]
    fn tolerant_of_missing_fields_and_bad_active() {
        let s = Session::from_json(r#"{"active": 9, "docs": [{"path": "x.txt"}], "future": 1}"#)
            .unwrap();
        assert_eq!(s.active, 0);
        assert_eq!(s.docs[0].path, Some(PathBuf::from("x.txt")));
        assert_eq!(s.version, VERSION);
    }

    #[test]
    fn rejects_invalid_json() {
        assert!(Session::from_json("{").is_err());
    }
}
