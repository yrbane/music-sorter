use crate::cache::{Cache, ProcessedEntry};
use anyhow::Result;
use colored::Colorize;
use std::path::Path;

/// Action déterminée pour une entrée à rollback.
#[derive(Debug, Clone, PartialEq)]
pub enum RollbackAction {
    /// dest existe et source existe → c'était une copie : on supprime juste la dest.
    DeleteDest,
    /// dest existe et source manque → c'était un --move : on remet dest à sa place d'origine.
    MoveBack,
    /// Ni source ni dest n'existent : entrée orpheline, à nettoyer du cache.
    AlreadyGone,
    /// dest n'existe pas mais source existe : rien à faire côté FS, juste nettoyer le cache.
    DestMissing,
}

/// Décide quoi faire pour une entrée donnée selon l'état actuel du filesystem.
pub fn decide_action(entry: &ProcessedEntry) -> RollbackAction {
    let src_exists = Path::new(&entry.source_path).exists();
    let dst_exists = Path::new(&entry.dest_path).exists();

    match (src_exists, dst_exists) {
        (true, true) => RollbackAction::DeleteDest,
        (false, true) => RollbackAction::MoveBack,
        (false, false) => RollbackAction::AlreadyGone,
        (true, false) => RollbackAction::DestMissing,
    }
}

/// Liste les entrées du cache (pour `--list-processed`).
pub fn print_list(cache: &Cache) -> Result<()> {
    let entries = cache.list_all_processed()?;
    if entries.is_empty() {
        println!("Aucune entrée dans le cache processed_files.");
        return Ok(());
    }
    println!("{} entrées dans le cache :\n", entries.len());
    for e in &entries {
        println!(
            "  [{}] {} → {}",
            e.status.dimmed(),
            e.source_path,
            e.dest_path
        );
    }
    Ok(())
}

/// Exécute (ou prévisualise) le rollback de toutes les entrées du cache.
/// `apply = false` → dry-run : on liste ce qui serait fait sans modifier le filesystem.
pub fn run(cache: &Cache, apply: bool) -> Result<RollbackSummary> {
    let entries = cache.list_all_processed()?;
    let mut summary = RollbackSummary::default();

    if entries.is_empty() {
        println!("Aucune entrée à défaire.");
        return Ok(summary);
    }

    let mode = if apply { "EXECUTION" } else { "DRY-RUN" };
    println!(
        "Mode {} : {} entrées à traiter\n",
        mode.bold(),
        entries.len()
    );

    for entry in &entries {
        let action = decide_action(entry);
        match action {
            RollbackAction::DeleteDest => {
                println!("  {} {} (copie)", "rm".cyan(), entry.dest_path);
                if apply {
                    if let Err(e) = std::fs::remove_file(&entry.dest_path) {
                        eprintln!("    {} {}", "✗".red().bold(), e);
                        summary.errors += 1;
                        continue;
                    }
                    let _ = cache.delete_processed(&entry.source_path);
                }
                summary.deleted += 1;
            }
            RollbackAction::MoveBack => {
                println!(
                    "  {} {} → {}",
                    "mv".cyan(),
                    entry.dest_path,
                    entry.source_path
                );
                if apply {
                    if let Some(parent) = Path::new(&entry.source_path).parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    if let Err(e) = std::fs::rename(&entry.dest_path, &entry.source_path) {
                        eprintln!("    {} {}", "✗".red().bold(), e);
                        summary.errors += 1;
                        continue;
                    }
                    let _ = cache.delete_processed(&entry.source_path);
                }
                summary.moved_back += 1;
            }
            RollbackAction::AlreadyGone => {
                println!(
                    "  {} {} (source et dest absentes, nettoyage du cache)",
                    "—".dimmed(),
                    entry.dest_path
                );
                if apply {
                    let _ = cache.delete_processed(&entry.source_path);
                }
                summary.cleaned += 1;
            }
            RollbackAction::DestMissing => {
                println!(
                    "  {} {} (dest absente, nettoyage du cache)",
                    "—".dimmed(),
                    entry.dest_path
                );
                if apply {
                    let _ = cache.delete_processed(&entry.source_path);
                }
                summary.cleaned += 1;
            }
        }
    }

    println!("\n{}", "Récapitulatif :".bold());
    println!(
        "  {} dest supprimées  {} fichiers replacés  {} entrées nettoyées  {} erreurs",
        summary.deleted, summary.moved_back, summary.cleaned, summary.errors
    );
    if !apply {
        println!(
            "\n{}",
            "Aucune action exécutée. Relance avec --apply pour appliquer."
                .yellow()
                .bold()
        );
    }

    Ok(summary)
}

#[derive(Debug, Default, PartialEq)]
pub struct RollbackSummary {
    pub deleted: usize,
    pub moved_back: usize,
    pub cleaned: usize,
    pub errors: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_decide_action_copy_case() {
        // source ET dest existent → c'était une copie
        let dir = tempdir().unwrap();
        let src = dir.path().join("a.mp3");
        let dst = dir.path().join("dst.mp3");
        fs::write(&src, b"x").unwrap();
        fs::write(&dst, b"x").unwrap();
        let entry = ProcessedEntry {
            source_path: src.to_string_lossy().into_owned(),
            dest_path: dst.to_string_lossy().into_owned(),
            status: "organized".into(),
            last_seen: 0,
        };
        assert_eq!(decide_action(&entry), RollbackAction::DeleteDest);
    }

    #[test]
    fn test_decide_action_move_case() {
        // source absente, dest existante → c'était un --move
        let dir = tempdir().unwrap();
        let dst = dir.path().join("dst.mp3");
        fs::write(&dst, b"x").unwrap();
        let entry = ProcessedEntry {
            source_path: "/nonexistent/source.mp3".into(),
            dest_path: dst.to_string_lossy().into_owned(),
            status: "organized".into(),
            last_seen: 0,
        };
        assert_eq!(decide_action(&entry), RollbackAction::MoveBack);
    }

    #[test]
    fn test_decide_action_orphan() {
        let entry = ProcessedEntry {
            source_path: "/nonexistent/a.mp3".into(),
            dest_path: "/nonexistent/dst.mp3".into(),
            status: "organized".into(),
            last_seen: 0,
        };
        assert_eq!(decide_action(&entry), RollbackAction::AlreadyGone);
    }

    #[test]
    fn test_run_dry_run_does_not_touch_filesystem() {
        let dir = tempdir().unwrap();
        let cache = Cache::open(dir.path()).unwrap();
        let src = dir.path().join("a.mp3");
        let dst = dir.path().join("dst.mp3");
        fs::write(&src, b"x").unwrap();
        fs::write(&dst, b"x").unwrap();
        cache
            .record_processed(
                src.to_str().unwrap(),
                1,
                1,
                Some(dst.to_str().unwrap()),
                "organized",
            )
            .unwrap();

        let summary = run(&cache, false).unwrap();
        assert_eq!(summary.deleted, 1);
        // dry-run : la dest doit toujours exister et le cache doit toujours contenir l'entrée
        assert!(dst.exists());
        assert_eq!(cache.list_all_processed().unwrap().len(), 1);
    }

    #[test]
    fn test_run_apply_deletes_dest_for_copy_case() {
        let dir = tempdir().unwrap();
        let cache = Cache::open(dir.path()).unwrap();
        let src = dir.path().join("a.mp3");
        let dst = dir.path().join("dst.mp3");
        fs::write(&src, b"x").unwrap();
        fs::write(&dst, b"x").unwrap();
        cache
            .record_processed(
                src.to_str().unwrap(),
                1,
                1,
                Some(dst.to_str().unwrap()),
                "organized",
            )
            .unwrap();

        let summary = run(&cache, true).unwrap();
        assert_eq!(summary.deleted, 1);
        assert!(!dst.exists(), "dest doit avoir été supprimée");
        assert!(src.exists(), "source intacte");
        assert_eq!(cache.list_all_processed().unwrap().len(), 0);
    }

    #[test]
    fn test_run_apply_moves_back_for_move_case() {
        let dir = tempdir().unwrap();
        let cache = Cache::open(dir.path()).unwrap();
        let src = dir.path().join("orig").join("a.mp3");
        let dst = dir.path().join("dst.mp3");
        fs::write(&dst, b"x").unwrap();
        // Pas de source : simule un --move déjà fait
        cache
            .record_processed(
                src.to_str().unwrap(),
                1,
                1,
                Some(dst.to_str().unwrap()),
                "organized",
            )
            .unwrap();

        let summary = run(&cache, true).unwrap();
        assert_eq!(summary.moved_back, 1);
        assert!(src.exists(), "source restaurée");
        assert!(!dst.exists(), "dest disparue");
        assert_eq!(cache.list_all_processed().unwrap().len(), 0);
    }
}
