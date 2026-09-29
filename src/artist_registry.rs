use crate::cache::Cache;

/// Renvoie le nom canonique pour cet artiste, en l'enregistrant s'il est nouveau
pub fn canonicalize(cache: &Cache, name: &str) -> anyhow::Result<String> {
    if let Some(existing) = cache.lookup_artist(name)? {
        return Ok(existing);
    }
    cache.record_artist(name, None)?;
    // Relire le nom stocké plutôt que de renvoyer `name` : en cas de course entre
    // workers, le premier writer gagne (INSERT OR IGNORE) et tous les appels
    // convergent vers la même casse. Sinon chaque worker garderait la sienne.
    Ok(cache
        .lookup_artist(name)?
        .unwrap_or_else(|| name.to_string()))
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

    /// Reproduit `--workers N` : plusieurs workers découvrent le même artiste
    /// (avec des casses différentes) au même instant. Tous doivent converger
    /// vers un unique nom canonique, sinon on obtient des dossiers dupliqués.
    #[test]
    fn test_canonicalize_converges_under_concurrent_first_sightings() {
        use std::collections::HashSet;
        use std::sync::{Arc, Barrier};
        use std::thread;

        for _round in 0..20 {
            let dir = tempdir().unwrap();
            let cache = Arc::new(Cache::open(dir.path()).unwrap());
            let n = 16;
            let barrier = Arc::new(Barrier::new(n));

            let handles: Vec<_> = (0..n)
                .map(|i| {
                    let cache = Arc::clone(&cache);
                    let barrier = Arc::clone(&barrier);
                    thread::spawn(move || {
                        let name = if i % 2 == 0 {
                            "Boards Of Canada"
                        } else {
                            "boards of canada"
                        };
                        barrier.wait();
                        canonicalize(&cache, name).unwrap()
                    })
                })
                .collect();

            let results: HashSet<String> = handles.into_iter().map(|h| h.join().unwrap()).collect();

            assert_eq!(
                results.len(),
                1,
                "les workers ont divergé sur la casse de l'artiste : {results:?}"
            );
        }
    }

    /// « The Future Sound of London » et « Future Sound of London » sont le même
    /// artiste : l'article initial est ignoré pour la clé du registre.
    #[test]
    fn test_leading_the_is_ignored() {
        let dir = tempdir().unwrap();
        let cache = Cache::open(dir.path()).unwrap();
        let a = canonicalize(&cache, "The Future Sound of London").unwrap();
        assert_eq!(a, "The Future Sound of London");
        let b = canonicalize(&cache, "Future Sound of London").unwrap();
        assert_eq!(b, "The Future Sound of London");
        let c = canonicalize(&cache, "future sound of london").unwrap();
        assert_eq!(c, "The Future Sound of London");
    }
}
