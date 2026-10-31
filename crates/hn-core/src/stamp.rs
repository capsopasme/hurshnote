//! 磁盘文件戳。

use serde::{Deserialize, Serialize};

/// 文件在某一时刻的状态。先比较修改时间与大小，不同时再比较内容哈希，
/// 以免"内容没变只是时间变了"（例如自己保存、外部 touch）误报修改。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct DiskStamp {
    /// 修改时间（Windows FILETIME，100ns 单位；其他平台为纳秒）
    pub mtime: u64,
    pub size: u64,
    pub hash: u64,
}

impl DiskStamp {
    pub fn new(mtime: u64, data: &[u8]) -> DiskStamp {
        DiskStamp {
            mtime,
            size: data.len() as u64,
            hash: hash_bytes(data),
        }
    }

    /// 时间与大小相同（快速判断，未读取内容）。
    pub fn same_meta(&self, mtime: u64, size: u64) -> bool {
        self.mtime == mtime && self.size == size
    }
}

/// 64 位 FNV-1a，每次处理 8 字节以提高速度。只用于变化检测，不用于安全目的。
pub fn hash_bytes(data: &[u8]) -> u64 {
    const PRIME: u64 = 0x0000_0100_0000_01B3;
    let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ data.len() as u64;
    let mut chunks = data.chunks_exact(8);
    for c in &mut chunks {
        h ^= u64::from_le_bytes(c.try_into().unwrap());
        h = h.wrapping_mul(PRIME);
        h ^= h >> 29;
    }
    for &b in chunks.remainder() {
        h ^= b as u64;
        h = h.wrapping_mul(PRIME);
    }
    h ^ (h >> 32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_differs() {
        assert_ne!(hash_bytes(b"abc"), hash_bytes(b"abd"));
        assert_ne!(hash_bytes(b""), hash_bytes(b"\0"));
        assert_ne!(hash_bytes(b"12345678a"), hash_bytes(b"12345678b"));
        assert_eq!(hash_bytes(b"same"), hash_bytes(b"same"));
    }

    #[test]
    fn stamp() {
        let s = DiskStamp::new(5, b"hello");
        assert_eq!(s.size, 5);
        assert!(s.same_meta(5, 5));
        assert!(!s.same_meta(6, 5));
    }
}
