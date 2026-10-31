//! 快照文件格式与快照目录。
//!
//! ```text
//! "HNSNAP01" | kind (1 字节：0 明文，1 加密) | 内容
//! ```
//! 明文：内容为 [`Payload`] 编码；加密：内容为 hn-crypto 文件（明文同样是 [`Payload`] 编码）。
//! 加密文档的快照用文档自己的密钥加密，恢复时需要密码。

use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use hn_crypto::Key;

use crate::atomic::write_atomic;
use crate::payload::{FormatError, Payload};

const MAGIC: &[u8; 8] = b"HNSNAP01";
const KIND_PLAIN: u8 = 0;
const KIND_ENCRYPTED: u8 = 1;
const EXT: &str = "snap";

#[derive(Debug)]
pub enum SnapshotError {
    Format(&'static str),
    Crypto(hn_crypto::Error),
    Io(std::io::Error),
}

impl fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SnapshotError::Format(e) => write!(f, "快照格式错误：{e}"),
            SnapshotError::Crypto(e) => write!(f, "{e}"),
            SnapshotError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for SnapshotError {}

impl From<FormatError> for SnapshotError {
    fn from(e: FormatError) -> Self {
        SnapshotError::Format(e.0)
    }
}

impl From<hn_crypto::Error> for SnapshotError {
    fn from(e: hn_crypto::Error) -> Self {
        SnapshotError::Crypto(e)
    }
}

impl From<std::io::Error> for SnapshotError {
    fn from(e: std::io::Error) -> Self {
        SnapshotError::Io(e)
    }
}

/// 生成快照文件字节。`key` 为 Some 时加密。
pub fn encode(payload: &Payload, key: Option<&Key>) -> Result<Vec<u8>, SnapshotError> {
    let body = payload.encode();
    let mut out = Vec::with_capacity(body.len() + 128);
    out.extend_from_slice(MAGIC);
    match key {
        None => {
            out.push(KIND_PLAIN);
            out.extend_from_slice(&body);
        }
        Some(k) => {
            out.push(KIND_ENCRYPTED);
            out.extend_from_slice(&hn_crypto::seal(k, &body)?);
        }
    }
    Ok(out)
}

/// 读出的快照：明文直接可用，加密的需要密钥。
#[derive(Debug)]
pub enum Snapshot {
    Plain(Payload),
    /// hn-crypto 文件字节
    Encrypted(Vec<u8>),
}

pub fn decode(bytes: &[u8]) -> Result<Snapshot, SnapshotError> {
    let rest = bytes
        .strip_prefix(MAGIC.as_slice())
        .ok_or(SnapshotError::Format("缺少文件标识"))?;
    let (&kind, body) = rest
        .split_first()
        .ok_or(SnapshotError::Format("文件过短"))?;
    match kind {
        KIND_PLAIN => Ok(Snapshot::Plain(Payload::decode(body)?)),
        KIND_ENCRYPTED => Ok(Snapshot::Encrypted(body.to_vec())),
        _ => Err(SnapshotError::Format("未知类型")),
    }
}

/// 解密加密快照。
pub fn open_encrypted(sealed: &[u8], key: &Key) -> Result<Payload, SnapshotError> {
    let pt = hn_crypto::open(key, sealed)?;
    Ok(Payload::decode(&pt)?)
}

/// 用密码解密加密快照，同时返回密钥。
pub fn open_encrypted_with_password(
    sealed: &[u8],
    password: &str,
) -> Result<(Key, Payload), SnapshotError> {
    let (key, pt) = hn_crypto::open_with_password(password, sealed)?;
    Ok((key, Payload::decode(&pt)?))
}

/// 快照目录。
#[derive(Debug, Clone)]
pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn new(dir: impl Into<PathBuf>) -> Store {
        Store { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// 新的快照名（不含目录）。
    pub fn new_name(&self) -> String {
        let mut r = [0u8; 8];
        if getrandom::getrandom(&mut r).is_err() {
            let t = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0);
            r = t.to_le_bytes();
        }
        let hex: String = r.iter().map(|b| format!("{b:02x}")).collect();
        format!("{hex}.{EXT}")
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    pub fn write(&self, name: &str, bytes: &[u8]) -> std::io::Result<()> {
        write_atomic(&self.path(name), bytes)
    }

    pub fn read(&self, name: &str) -> Result<Snapshot, SnapshotError> {
        decode(&fs::read(self.path(name))?)
    }

    pub fn remove(&self, name: &str) {
        let _ = fs::remove_file(self.path(name));
    }

    /// 删除不在 `keep` 中的快照（以及残留的临时文件）。返回删除数量。
    pub fn cleanup(&self, keep: &HashSet<String>) -> usize {
        let Ok(rd) = fs::read_dir(&self.dir) else {
            return 0;
        };
        let mut n = 0;
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let is_snap = name.ends_with(&format!(".{EXT}"));
            let is_tmp = name.contains(&format!(".{EXT}.")) && name.ends_with(".tmp");
            if ((is_snap && !keep.contains(&name)) || is_tmp) && fs::remove_file(e.path()).is_ok() {
                n += 1;
            }
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hn_crypto::KdfParams;

    const FAST: KdfParams = KdfParams {
        m_cost_kib: 256,
        t_cost: 1,
        p_cost: 1,
    };

    #[test]
    fn plain_roundtrip() {
        let p = Payload::new(b"hello".to_vec()).with("enc", "utf-8");
        match decode(&encode(&p, None).unwrap()).unwrap() {
            Snapshot::Plain(d) => assert_eq!(d, p),
            _ => panic!(),
        }
    }

    #[test]
    fn encrypted_roundtrip() {
        let key = Key::generate("pw", FAST).unwrap();
        let p = Payload::new("机密".as_bytes().to_vec());
        let bytes = encode(&p, Some(&key)).unwrap();
        // 明文不得出现在快照中
        assert!(!bytes.windows(6).any(|w| w == "机密".as_bytes()));
        let Snapshot::Encrypted(sealed) = decode(&bytes).unwrap() else {
            panic!()
        };
        assert_eq!(open_encrypted(&sealed, &key).unwrap(), p);
        let (_, p2) = open_encrypted_with_password(&sealed, "pw").unwrap();
        assert_eq!(p2, p);
        assert!(open_encrypted_with_password(&sealed, "bad").is_err());
    }

    #[test]
    fn store_and_cleanup() {
        let dir = std::env::temp_dir().join(format!("hn-snap-{}", std::process::id()));
        let s = Store::new(&dir);
        let a = s.new_name();
        let b = s.new_name();
        assert_ne!(a, b);
        s.write(&a, &encode(&Payload::new(b"a".to_vec()), None).unwrap())
            .unwrap();
        s.write(&b, &encode(&Payload::new(b"b".to_vec()), None).unwrap())
            .unwrap();
        let keep: HashSet<String> = [a.clone()].into_iter().collect();
        assert_eq!(s.cleanup(&keep), 1);
        assert!(s.path(&a).exists());
        assert!(!s.path(&b).exists());
        assert!(matches!(s.read(&a).unwrap(), Snapshot::Plain(_)));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_garbage() {
        assert!(decode(b"xx").is_err());
        assert!(decode(b"HNSNAP01").is_err());
        assert!(decode(b"HNSNAP01\x07abc").is_err());
    }
}
