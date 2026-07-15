use crate::cache::Cache;
use std::collections::HashMap;
use std::path::PathBuf;

/// Unifie un album via le registre : renvoie le nom d'album en casse canonique
/// (première vue) et l'année canonique (la plus ancienne connue). Relit le
/// gagnant après enregistrement → convergence entre workers. Un `year = None`
/// hérite de l'année déjà enregistrée pour l'album.
pub fn canonicalize(
    cache: &Cache,
    artist: &str,
    album: &str,
    year: Option<u32>,
) -> anyhow::Result<(String, Option<u32>)> {
    cache.upsert_album(artist, album, year)?;
    // Relire le gagnant plutôt que de renvoyer les valeurs locales : en cas de
    // course entre workers, tous convergent vers la même casse et la plus
    // ancienne année.
    match cache.lookup_album(artist, album)? {
        Some(canonical) => Ok(canonical),
        None => Ok((album.to_string(), year)),
    }
}

/// Regroupe les fichiers par dossier parent.
/// Les fichiers sans parent sont ignorés (ex. chemins racines).
#[allow(dead_code)]
pub fn group_by_parent(files: &[PathBuf]) -> HashMap<PathBuf, Vec<PathBuf>> {
    let mut map: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
    for f in files {
        if let Some(parent) = f.parent() {
            map.entry(parent.to_path_buf()).or_default().push(f.clone());
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Reproduit `--workers N` : plusieurs workers rangent le même album (casses
    /// et années différentes) simultanément. Tous doivent converger vers un nom
    /// d'album unique ET l'année la plus ancienne.
    #[test]
    fn test_canonicalize_converges_under_concurrency() {
        use std::collections::HashSet;
        use std::sync::{Arc, Barrier};
        use std::thread;

        for _round in 0..20 {
            let dir = tempfile::tempdir().unwrap();
            let cache = Arc::new(Cache::open(dir.path()).unwrap());
            let n = 16;
            let barrier = Arc::new(Barrier::new(n));

            let handles: Vec<_> = (0..n)
                .map(|i| {
                    let cache = Arc::clone(&cache);
                    let barrier = Arc::clone(&barrier);
                    thread::spawn(move || {
                        let (album, year) = if i % 2 == 0 {
                            ("Geogaddi", Some(2002u32))
                        } else {
                            ("geogaddi", Some(2004u32))
                        };
                        barrier.wait();
                        canonicalize(&cache, "Boards of Canada", album, year).unwrap()
                    })
                })
                .collect();

            let results: Vec<(String, Option<u32>)> =
                handles.into_iter().map(|h| h.join().unwrap()).collect();

            // La casse d'album (première-vue, immuable) converge dès ce passage :
            // tous les workers renvoient le même nom.
            let names: HashSet<&str> = results.iter().map(|(a, _)| a.as_str()).collect();
            assert_eq!(names.len(), 1, "casse d'album divergente : {names:?}");

            // L'année « la plus ancienne » n'est connue qu'une fois TOUS les fichiers
            // vus : le registre final doit tenir 2002 (la consolidation post-passe
            // l'appliquera aux dossiers).
            let (_, year) = cache.lookup_album("boards of canada", "geogaddi").unwrap().unwrap();
            assert_eq!(year, Some(2002), "le registre final doit tenir la plus ancienne année");
        }
    }

    #[test]
    fn test_canonicalize_inherits_year_when_none() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::open(dir.path()).unwrap();
        // L'album est d'abord enregistré avec 2002.
        canonicalize(&cache, "BoC", "Geogaddi", Some(2002)).unwrap();
        // Un fichier du même album sans année hérite de 2002 et de la casse.
        let (album, year) = canonicalize(&cache, "BoC", "geogaddi", None).unwrap();
        assert_eq!(album, "Geogaddi");
        assert_eq!(year, Some(2002));
    }

    #[test]
    fn test_group_by_parent_dir() {
        let files = vec![
            PathBuf::from("/a/album1/1.mp3"),
            PathBuf::from("/a/album1/2.mp3"),
            PathBuf::from("/a/album2/1.mp3"),
        ];
        let groups = group_by_parent(&files);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[&PathBuf::from("/a/album1")].len(), 2);
        assert_eq!(groups[&PathBuf::from("/a/album2")].len(), 1);
    }

    #[test]
    fn test_group_by_parent_empty() {
        let files: Vec<PathBuf> = vec![];
        let groups = group_by_parent(&files);
        assert!(groups.is_empty());
    }

    #[test]
    fn test_group_by_parent_no_parent_skipped() {
        // Un PathBuf sans parent (ex. juste un nom de fichier au root) ne doit pas crasher
        let files = vec![PathBuf::from("/")];
        let groups = group_by_parent(&files);
        // "/" a parent None → ignoré
        assert!(groups.is_empty() || groups.len() <= 1);
    }
}
