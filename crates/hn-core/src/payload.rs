//! 文档内容 + 元数据的布局（加密文档的明文、快照内容共用）。
//!
//! ```text
//! "HNDOC1\n"
//! key=value\n      （零或多行；值中不含换行）
//! \n               （空行结束元数据）
//! <UTF-8 文本字节>
//! ```

use std::fmt;

const MAGIC: &[u8] = b"HNDOC1\n";

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Payload {
    pub meta: Vec<(String, String)>,
    pub text: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatError(pub &'static str);

impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "文档数据格式错误：{}", self.0)
    }
}

impl std::error::Error for FormatError {}

impl Payload {
    pub fn new(text: Vec<u8>) -> Payload {
        Payload {
            meta: Vec::new(),
            text,
        }
    }

    pub fn with(mut self, key: &str, value: &str) -> Payload {
        self.set(key, value);
        self
    }

    pub fn set(&mut self, key: &str, value: &str) {
        let v = value.replace(['\r', '\n'], " ");
        match self.meta.iter_mut().find(|(k, _)| k == key) {
            Some(e) => e.1 = v,
            None => self.meta.push((key.to_string(), v)),
        }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.meta
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.text.len() + 128);
        out.extend_from_slice(MAGIC);
        for (k, v) in &self.meta {
            out.extend_from_slice(k.as_bytes());
            out.push(b'=');
            out.extend_from_slice(v.as_bytes());
            out.push(b'\n');
        }
        out.push(b'\n');
        out.extend_from_slice(&self.text);
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Payload, FormatError> {
        let rest = bytes
            .strip_prefix(MAGIC)
            .ok_or(FormatError("缺少文件标识"))?;
        let mut meta = Vec::new();
        let mut pos = 0;
        loop {
            let nl = rest[pos..]
                .iter()
                .position(|&b| b == b'\n')
                .ok_or(FormatError("元数据未结束"))?;
            let line = &rest[pos..pos + nl];
            pos += nl + 1;
            if line.is_empty() {
                break;
            }
            let line = std::str::from_utf8(line).map_err(|_| FormatError("元数据不是 UTF-8"))?;
            let (k, v) = line.split_once('=').ok_or(FormatError("元数据缺少等号"))?;
            meta.push((k.to_string(), v.to_string()));
        }
        Ok(Payload {
            meta,
            text: rest[pos..].to_vec(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let p = Payload::new("正文\n\nkey=notmeta".as_bytes().to_vec())
            .with("eol", "crlf")
            .with("lang", "python")
            .with("evil", "a\nb");
        let d = Payload::decode(&p.encode()).unwrap();
        assert_eq!(d.get("eol"), Some("crlf"));
        assert_eq!(d.get("evil"), Some("a b"));
        assert_eq!(d.text, p.text);
    }

    #[test]
    fn empty() {
        let p = Payload::default();
        assert_eq!(Payload::decode(&p.encode()).unwrap(), p);
    }

    #[test]
    fn errors() {
        assert!(Payload::decode(b"nope").is_err());
        assert!(Payload::decode(b"HNDOC1\nk=v").is_err());
        assert!(Payload::decode(b"HNDOC1\nnoequals\n\n").is_err());
    }
}
