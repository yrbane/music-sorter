use anyhow::Result;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::Read;
use std::path::Path;

/// Hash SHA-256 du contenu binaire d'un fichier (en hex)
pub fn content_hash(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 { break; }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Normalise une chaîne pour servir de clé de cache API
pub fn api_key(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_content_hash_stable() {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(b"hello").unwrap();
        let h1 = content_hash(f.path()).unwrap();
        let h2 = content_hash(f.path()).unwrap();
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64);
    }

    #[test]
    fn test_content_hash_differs_per_content() {
        let mut a = NamedTempFile::new().unwrap();
        a.write_all(b"foo").unwrap();
        let mut b = NamedTempFile::new().unwrap();
        b.write_all(b"bar").unwrap();
        assert_ne!(content_hash(a.path()).unwrap(), content_hash(b.path()).unwrap());
    }

    #[test]
    fn test_api_key_normalizes_case_and_whitespace() {
        assert_eq!(api_key("  Hello   World  "), api_key("hello world"));
    }
}
