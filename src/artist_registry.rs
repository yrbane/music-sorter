use crate::cache::Cache;

/// Renvoie le nom canonique pour cet artiste, en l'enregistrant s'il est nouveau
pub fn canonicalize(cache: &Cache, name: &str) -> anyhow::Result<String> {
    if let Some(existing) = cache.lookup_artist(name)? {
        return Ok(existing);
    }
    cache.record_artist(name, None)?;
    Ok(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_first_seen_wins() {
        let dir = tempdir().unwrap();
        let cache = Cache::open(dir.path()).unwrap();
        let a = canonicalize(&cache, "Boards Of Canada").unwrap();
        assert_eq!(a, "Boards Of Canada");
        let b = canonicalize(&cache, "boards of canada").unwrap();
        assert_eq!(b, "Boards Of Canada");
    }
}
