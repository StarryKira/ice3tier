//! 文件的大小和 SHA-256。

use std::fs::File;
use std::io::{self, Write};
use std::path::Path;

use i3t_model::update::{Artifact, Sha256};
use ring::digest::{Context, SHA256};

/// 一个文件实际的大小和 SHA-256，用来和清单里写的比。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileDigest {
    pub size: u64,
    pub sha256: Sha256,
}

impl FileDigest {
    /// 把文件从头读一遍算出来。这是阻塞的调用，文件有几十 MB，异步代码里要放到阻塞线程上跑。
    ///
    /// # Errors
    ///
    /// 文件打不开或者读不了。
    pub fn of_file(path: &Path) -> io::Result<Self> {
        let mut hasher = Hasher(Context::new(&SHA256));
        let size = io::copy(&mut File::open(path)?, &mut hasher)?;
        let sha256: [u8; Sha256::LEN] = hasher.0.finish().as_ref().try_into().expect("SHA-256 的结果固定是 32 字节，这是算法的定义");
        Ok(Self { size, sha256: Sha256::from_bytes(sha256) })
    }

    #[must_use]
    pub fn matches(&self, artifact: &Artifact) -> bool {
        self.size == artifact.size && self.sha256 == artifact.sha256
    }
}

/// 让 [`io::copy`] 把读到的字节直接喂给哈希。
struct Hasher(Context);

impl Write for Hasher {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_matches_the_published_test_vector() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("abc");
        std::fs::write(&path, b"abc").unwrap();
        let digest = FileDigest::of_file(&path).unwrap();
        assert_eq!(digest.size, 3);
        // FIPS 180-2 附录 B.1 的例子。
        assert_eq!(digest.sha256.to_string(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn a_file_matches_only_an_artifact_with_the_same_size_and_hash() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file");
        std::fs::write(&path, vec![7; 200_000]).unwrap();
        let digest = FileDigest::of_file(&path).unwrap();
        let artifact = Artifact { file: "file".to_owned(), size: 200_000, sha256: digest.sha256 };
        assert!(digest.matches(&artifact));
        assert!(!digest.matches(&Artifact { size: 200_001, ..artifact.clone() }));
        assert!(!digest.matches(&Artifact { sha256: Sha256::from_bytes([0; 32]), ..artifact }));
        assert!(FileDigest::of_file(&dir.path().join("missing")).is_err());
    }
}
