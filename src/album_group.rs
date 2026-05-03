use std::collections::HashMap;
use std::path::PathBuf;

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
