//! HurshNote 加密文件格式 v1。
//!
//! ```text
//! 偏移  长度  字段
//! 0     8     magic = "HURSHENC"
//! 8     2     format_version = 1 (LE)
//! 10    1     kdf_id = 1 (Argon2id)
//! 11    4     argon2 m_cost，KiB (LE)
//! 15    4     argon2 t_cost (LE)
//! 19    1     argon2 p_cost
//! 20    16    salt
//! 36    1     cipher_id = 1 (AES-256-GCM)
//! 37    12    nonce
//! 49    ...   ciphertext || 16 字节 tag
//! ```
//!
//! 整个头部作为 AEAD 附加认证数据，任何字段被篡改都会解密失败。
//! 密钥由密码经 Argon2id 派生；同一份 [`Key`]（同一 salt）可以多次加密，每次使用新的随机 nonce。

use std::fmt;

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use argon2::{Algorithm, Argon2, Params, Version};
use zeroize::Zeroizing;

pub const MAGIC: &[u8; 8] = b"HURSHENC";
pub const VERSION: u16 = 1;
pub const HEADER_LEN: usize = 49;
const KDF_ARGON2ID: u8 = 1;
const CIPHER_AES256GCM: u8 = 1;
const TAG_LEN: usize = 16;

/// 读取文件时接受的 KDF 参数上限（防止恶意文件耗尽内存或 CPU）。
const MAX_M_COST_KIB: u32 = 1024 * 1024; // 1 GiB
const MAX_T_COST: u32 = 64;
const MAX_P_COST: u8 = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// 不是加密文件（魔数不符）。
    NotEncrypted,
    UnsupportedVersion(u16),
    /// 头部损坏或参数超出范围。
    BadHeader(&'static str),
    /// 密码错误或文件被篡改/损坏（GCM 无法区分二者）。
    WrongPasswordOrCorrupt,
    Kdf(String),
    Random(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotEncrypted => write!(f, "不是 HurshNote 加密文件"),
            Error::UnsupportedVersion(v) => write!(f, "不支持的加密文件版本 {v}，请升级 HurshNote"),
            Error::BadHeader(why) => write!(f, "加密文件头部损坏（{why}）"),
            Error::WrongPasswordOrCorrupt => write!(f, "密码错误，或文件已损坏"),
            Error::Kdf(e) => write!(f, "密钥派生失败：{e}"),
            Error::Random(e) => write!(f, "无法获取随机数：{e}"),
        }
    }
}

impl std::error::Error for Error {}

/// Argon2id 参数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KdfParams {
    pub m_cost_kib: u32,
    pub t_cost: u32,
    pub p_cost: u8,
}

impl Default for KdfParams {
    /// 64 MiB、3 轮、单通道：在普通电脑上约 0.2–0.5 秒。
    fn default() -> Self {
        KdfParams {
            m_cost_kib: 64 * 1024,
            t_cost: 3,
            p_cost: 1,
        }
    }
}

impl KdfParams {
    fn validate(&self) -> Result<(), Error> {
        if self.m_cost_kib < 8 * self.p_cost as u32 || self.m_cost_kib > MAX_M_COST_KIB {
            return Err(Error::BadHeader("内存参数超出范围"));
        }
        if self.t_cost == 0 || self.t_cost > MAX_T_COST {
            return Err(Error::BadHeader("迭代参数超出范围"));
        }
        if self.p_cost == 0 || self.p_cost > MAX_P_COST {
            return Err(Error::BadHeader("并行参数超出范围"));
        }
        Ok(())
    }
}

/// 由密码派生出的密钥及其 salt、参数。内存中的密钥在释放时清零。
#[derive(Clone)]
pub struct Key {
    key: Zeroizing<[u8; 32]>,
    salt: [u8; 16],
    params: KdfParams,
}

impl fmt::Debug for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Key")
            .field("params", &self.params)
            .finish_non_exhaustive()
    }
}

fn random<const N: usize>() -> Result<[u8; N], Error> {
    let mut buf = [0u8; N];
    getrandom::getrandom(&mut buf).map_err(|e| Error::Random(e.to_string()))?;
    Ok(buf)
}

impl Key {
    /// 用给定 salt 派生密钥。
    pub fn derive(password: &str, salt: [u8; 16], params: KdfParams) -> Result<Key, Error> {
        params.validate()?;
        let p = Params::new(
            params.m_cost_kib,
            params.t_cost,
            params.p_cost as u32,
            Some(32),
        )
        .map_err(|e| Error::Kdf(e.to_string()))?;
        let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, p);
        let mut key = Zeroizing::new([0u8; 32]);
        argon
            .hash_password_into(password.as_bytes(), &salt, key.as_mut())
            .map_err(|e| Error::Kdf(e.to_string()))?;
        Ok(Key { key, salt, params })
    }

    /// 为新文件（或修改密码）生成随机 salt 并派生密钥。
    pub fn generate(password: &str, params: KdfParams) -> Result<Key, Error> {
        Key::derive(password, random()?, params)
    }

    pub fn params(&self) -> KdfParams {
        self.params
    }

    /// 密码是否与这个密钥对应（用同一 salt 与参数重新派生后按常数时间比较）。修改密码前用来确认旧密码。
    pub fn verify(&self, password: &str) -> Result<bool, Error> {
        let other = Key::derive(password, self.salt, self.params)?;
        let diff = self
            .key
            .iter()
            .zip(other.key.iter())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b));
        Ok(diff == 0)
    }
}

/// 解析出的头部。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub params: KdfParams,
    pub salt: [u8; 16],
    pub nonce: [u8; 12],
}

/// 是否以加密文件魔数开头。
pub fn is_encrypted(bytes: &[u8]) -> bool {
    bytes.starts_with(MAGIC)
}

pub fn parse_header(bytes: &[u8]) -> Result<Header, Error> {
    if !is_encrypted(bytes) {
        return Err(Error::NotEncrypted);
    }
    if bytes.len() < HEADER_LEN + TAG_LEN {
        return Err(Error::BadHeader("文件过短"));
    }
    let version = u16::from_le_bytes([bytes[8], bytes[9]]);
    if version != VERSION {
        return Err(Error::UnsupportedVersion(version));
    }
    if bytes[10] != KDF_ARGON2ID {
        return Err(Error::BadHeader("未知的密钥派生算法"));
    }
    let params = KdfParams {
        m_cost_kib: u32::from_le_bytes(bytes[11..15].try_into().unwrap()),
        t_cost: u32::from_le_bytes(bytes[15..19].try_into().unwrap()),
        p_cost: bytes[19],
    };
    params.validate()?;
    if bytes[36] != CIPHER_AES256GCM {
        return Err(Error::BadHeader("未知的加密算法"));
    }
    Ok(Header {
        params,
        salt: bytes[20..36].try_into().unwrap(),
        nonce: bytes[37..49].try_into().unwrap(),
    })
}

fn build_header(key: &Key, nonce: &[u8; 12]) -> [u8; HEADER_LEN] {
    let mut h = [0u8; HEADER_LEN];
    h[..8].copy_from_slice(MAGIC);
    h[8..10].copy_from_slice(&VERSION.to_le_bytes());
    h[10] = KDF_ARGON2ID;
    h[11..15].copy_from_slice(&key.params.m_cost_kib.to_le_bytes());
    h[15..19].copy_from_slice(&key.params.t_cost.to_le_bytes());
    h[19] = key.params.p_cost;
    h[20..36].copy_from_slice(&key.salt);
    h[36] = CIPHER_AES256GCM;
    h[37..49].copy_from_slice(nonce);
    h
}

/// 加密明文，返回完整文件字节。每次调用使用新的随机 nonce。
pub fn seal(key: &Key, plaintext: &[u8]) -> Result<Vec<u8>, Error> {
    let nonce: [u8; 12] = random()?;
    let header = build_header(key, &nonce);
    let cipher = Aes256Gcm::new_from_slice(key.key.as_ref()).expect("密钥长度固定为 32");
    let ct = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad: &header,
            },
        )
        .map_err(|_| Error::WrongPasswordOrCorrupt)?;
    let mut out = Vec::with_capacity(HEADER_LEN + ct.len());
    out.extend_from_slice(&header);
    out.extend_from_slice(&ct);
    Ok(out)
}

/// 用已有密钥解密。密钥的 salt / 参数必须与文件一致。
pub fn open(key: &Key, bytes: &[u8]) -> Result<Zeroizing<Vec<u8>>, Error> {
    let h = parse_header(bytes)?;
    if h.salt != key.salt || h.params != key.params {
        return Err(Error::WrongPasswordOrCorrupt);
    }
    let cipher = Aes256Gcm::new_from_slice(key.key.as_ref()).expect("密钥长度固定为 32");
    let pt = cipher
        .decrypt(
            Nonce::from_slice(&h.nonce),
            Payload {
                msg: &bytes[HEADER_LEN..],
                aad: &bytes[..HEADER_LEN],
            },
        )
        .map_err(|_| Error::WrongPasswordOrCorrupt)?;
    Ok(Zeroizing::new(pt))
}

/// 用密码解密：先按文件头部派生密钥。返回密钥（供之后保存复用）与明文。
pub fn open_with_password(
    password: &str,
    bytes: &[u8],
) -> Result<(Key, Zeroizing<Vec<u8>>), Error> {
    let h = parse_header(bytes)?;
    let key = Key::derive(password, h.salt, h.params)?;
    let pt = open(&key, bytes)?;
    Ok((key, pt))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用的小参数，避免测试太慢。
    const FAST: KdfParams = KdfParams {
        m_cost_kib: 256,
        t_cost: 1,
        p_cost: 1,
    };

    #[test]
    fn roundtrip() {
        let key = Key::generate("密码 pass", FAST).unwrap();
        let data = "机密内容\r\nsecret".as_bytes();
        let file = seal(&key, data).unwrap();
        assert!(is_encrypted(&file));
        assert_eq!(file.len(), HEADER_LEN + data.len() + TAG_LEN);
        assert_eq!(&open(&key, &file).unwrap()[..], data);
        let (k2, pt) = open_with_password("密码 pass", &file).unwrap();
        assert_eq!(&pt[..], data);
        // 复用派生出的密钥再次保存，仍可用密码打开
        let file2 = seal(&k2, b"v2").unwrap();
        assert_eq!(
            &open_with_password("密码 pass", &file2).unwrap().1[..],
            b"v2"
        );
    }

    #[test]
    fn empty_plaintext() {
        let key = Key::generate("x", FAST).unwrap();
        let file = seal(&key, b"").unwrap();
        assert_eq!(&open(&key, &file).unwrap()[..], b"");
    }

    #[test]
    fn nonce_changes_every_seal() {
        let key = Key::generate("x", FAST).unwrap();
        let a = seal(&key, b"same").unwrap();
        let b = seal(&key, b"same").unwrap();
        assert_ne!(a[37..49], b[37..49]);
        assert_ne!(a, b);
    }

    #[test]
    fn wrong_password() {
        let key = Key::generate("right", FAST).unwrap();
        let file = seal(&key, b"data").unwrap();
        assert_eq!(
            open_with_password("wrong", &file).err(),
            Some(Error::WrongPasswordOrCorrupt)
        );
    }

    #[test]
    fn known_salt_is_deterministic() {
        let a = Key::derive("pw", [7; 16], FAST).unwrap();
        let b = Key::derive("pw", [7; 16], FAST).unwrap();
        assert_eq!(*a.key, *b.key);
        let c = Key::derive("pw", [8; 16], FAST).unwrap();
        assert_ne!(*a.key, *c.key);
    }

    #[test]
    fn header_fields() {
        let key = Key::generate("x", FAST).unwrap();
        let file = seal(&key, b"abc").unwrap();
        let h = parse_header(&file).unwrap();
        assert_eq!(h.params, FAST);
        assert_eq!(h.salt, key.salt);
    }

    #[test]
    fn rejects_bad_headers() {
        assert_eq!(parse_header(b"plain text").err(), Some(Error::NotEncrypted));
        let key = Key::generate("x", FAST).unwrap();
        let mut file = seal(&key, b"abc").unwrap();
        file[8] = 2;
        assert_eq!(
            parse_header(&file).err(),
            Some(Error::UnsupportedVersion(2))
        );
        file[8] = 1;
        file[11..15].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(parse_header(&file), Err(Error::BadHeader(_))));
        assert!(matches!(
            parse_header(&file[..20]),
            Err(Error::BadHeader(_))
        ));
    }

    /// 简单的变异测试：随机翻转字节或截断，必须返回错误而不是 panic 或得到错误明文。
    #[test]
    fn tampering_is_detected() {
        let key = Key::generate("pw", FAST).unwrap();
        let data = b"The quick brown fox jumps over the lazy dog".repeat(10);
        let file = seal(&key, &data).unwrap();
        let mut rng: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            rng
        };
        for _ in 0..500 {
            let mut f = file.clone();
            match next() % 3 {
                0 => {
                    let i = (next() as usize) % f.len();
                    f[i] ^= 1 << (next() % 8);
                }
                1 => {
                    let n = (next() as usize) % f.len();
                    f.truncate(n);
                }
                _ => {
                    let i = (next() as usize) % f.len();
                    f.insert(i, next() as u8);
                }
            }
            if f == file {
                continue;
            }
            assert!(open(&key, &f).is_err());
        }
    }

    #[test]
    fn verify_password() {
        let k = Key::generate("正确的密码", FAST).unwrap();
        assert!(k.verify("正确的密码").unwrap());
        assert!(!k.verify("错误的密码").unwrap());
        assert!(!k.verify("").unwrap());
        // 换了 salt 的同一密码不是同一个密钥
        let k2 = Key::generate("正确的密码", FAST).unwrap();
        assert!(k2.verify("正确的密码").unwrap());
        assert_ne!(k.salt, k2.salt);
    }
}
