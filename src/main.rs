mod artist_registry;
mod cache;
mod cache_keys;
mod cli;
mod config;
mod coverart;
mod discogs;
mod enricher;
mod fingerprint;
mod models;
mod musicbrainz;
mod organizer;
mod rate_limiter;
mod retry;
mod scanner;
mod tags;

use crate::enricher::Enricher;
use crate::models::ProcessResult;
use anyhow::Result;
use clap::Parser;
use colored::*;
use std::sync::Arc;

fn main() -> Result<()> {
    let config = config::Config::load()?;
    let args = cli::Args::parse().resolve(&config);

    println!("{} {}", "Source:".bold(), args.source.display());
    println!("{} {}", "Destination:".bold(), args.target.display());

    if !args.source.exists() {
        eprintln!("{} Le dossier source n'existe pas : {}", "✗".red().bold(), args.source.display());
        std::process::exit(1);
    }

    // Création de la destination + ouverture du cache SQLite partagé
    std::fs::create_dir_all(&args.target)?;
    let cache = Arc::new(cache::Cache::open(&args.target)?);

    let files = scanner::scan(&args.source);
    println!("\n{} fichiers audio trouvés\n", files.len().to_string().bold());

    if files.is_empty() {
        println!("Rien à faire.");
        return Ok(());
    }

    let enricher = Enricher::new(&config, cache.clone())?;

    let results: Vec<ProcessResult> = if args.workers > 1 {
        process_parallel(
            &files,
            &enricher,
            &cache,
            &args.source,
            &args.target,
            args.do_move,
            args.workers,
        )
    } else {
        process_sequential(&files, &enricher, &cache, &args.source, &args.target, args.do_move)
    };

    print_summary(&results);
    Ok(())
}

fn process_sequential(
    files: &[std::path::PathBuf],
    enricher: &Enricher,
    cache: &Arc<cache::Cache>,
    source: &std::path::Path,
    target: &std::path::Path,
    do_move: bool,
) -> Vec<ProcessResult> {
    files
        .iter()
        .map(|file| process_file(file, enricher, cache, source, target, do_move))
        .collect()
}

fn process_parallel(
    files: &[std::path::PathBuf],
    enricher: &Enricher,
    cache: &Arc<cache::Cache>,
    source: &std::path::Path,
    target: &std::path::Path,
    do_move: bool,
    workers: usize,
) -> Vec<ProcessResult> {
    use rayon::prelude::*;

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(workers)
        .build()
        .expect("Impossible de créer le pool de threads");

    pool.install(|| {
        files
            .par_iter()
            .map(|file| process_file(file, enricher, cache, source, target, do_move))
            .collect()
    })
}

fn process_file(
    file: &std::path::Path,
    enricher: &Enricher,
    cache: &Arc<cache::Cache>,
    source: &std::path::Path,
    target: &std::path::Path,
    do_move: bool,
) -> ProcessResult {
    let filename = file.file_name().unwrap_or_default().to_string_lossy();

    // Lecture des métadonnées du fichier source pour clé de cache (mtime + size)
    let metadata = match std::fs::metadata(file) {
        Ok(m) => m,
        Err(_) => {
            return ProcessResult::Error {
                path: file.to_path_buf(),
                reason: "Impossible de lire les métadonnées du fichier".into(),
            };
        }
    };
    let mtime: Option<i64> = metadata.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64);
    let size = metadata.len() as i64;
    let source_str = file.to_string_lossy().into_owned();

    // Skip total si déjà organisé et la destination existe encore (uniquement si mtime disponible)
    if let Some(mt) = mtime {
        if let Ok(Some((status, dest_opt))) = cache.lookup_processed(&source_str, mt, size) {
            if status == "organized" {
                if let Some(d) = dest_opt {
                    let dest_pb = std::path::PathBuf::from(d);
                    if dest_pb.exists() {
                        println!("  {} {} (cache)", "—".dimmed(), filename);
                        return ProcessResult::Organized {
                            from: file.to_path_buf(),
                            to: dest_pb,
                        };
                    }
                }
            }
        }
    }

    let info = match enricher.enrich(file) {
        Ok(info) => info,
        Err(e) => {
            eprintln!("  {} {} — {}", "✗".red().bold(), filename, e);
            if let Some(mt) = mtime {
                let _ = cache.record_processed(&source_str, mt, size, None, "error");
            }
            return ProcessResult::Error { path: file.to_path_buf(), reason: e.to_string() };
        }
    };

    let dest = organizer::build_destination_path(target, &info, file, source);

    match organizer::copy_to_destination(file, &dest) {
        Ok(organizer::CopyResult::Copied) => {
            if let Err(e) = tags::write_tags(&dest, &info) {
                eprintln!("  {} {} — Copié mais erreur tags : {}", "⚠".yellow().bold(), filename, e);
            }

            let is_unsorted = dest.to_string_lossy().contains("_unsorted");
            if is_unsorted {
                println!("  {} {} → _unsorted/", "⚠".yellow().bold(), filename);
            } else {
                println!("  {} {} → {}", "✓".green().bold(), filename, dest.display());
            }
            if do_move { let _ = organizer::remove_source(file); }

            if is_unsorted {
                if let Some(mt) = mtime {
                    let _ = cache.record_processed(&source_str, mt, size, Some(&dest.to_string_lossy()), "unsorted");
                }
                ProcessResult::Unsorted { from: file.to_path_buf(), to: dest }
            } else {
                if let Some(mt) = mtime {
                    let _ = cache.record_processed(&source_str, mt, size, Some(&dest.to_string_lossy()), "organized");
                }
                ProcessResult::Organized { from: file.to_path_buf(), to: dest }
            }
        }
        Ok(organizer::CopyResult::Replaced { bitrate }) => {
            if let Err(e) = tags::write_tags(&dest, &info) {
                eprintln!("  ⚠ Erreur écriture tags après remplacement : {}", e);
            }
            println!("  {} {} — remplacé ({}kbps)", "↑".cyan().bold(), filename, bitrate);
            if do_move { let _ = organizer::remove_source(file); }
            if let Some(mt) = mtime {
                let _ = cache.record_processed(&source_str, mt, size, Some(&dest.to_string_lossy()), "conflict");
            }
            ProcessResult::ConflictResolved { path: dest, kept_bitrate: bitrate }
        }
        Ok(organizer::CopyResult::Skipped { existing_bitrate }) => {
            println!("  {} {} — ignoré (existant : {}kbps)", "—".dimmed(), filename, existing_bitrate);
            if let Some(mt) = mtime {
                let _ = cache.record_processed(&source_str, mt, size, Some(&dest.to_string_lossy()), "organized");
            }
            ProcessResult::Organized { from: file.to_path_buf(), to: dest }
        }
        Err(e) => {
            eprintln!("  {} {} — {}", "✗".red().bold(), filename, e);
            if let Some(mt) = mtime {
                let _ = cache.record_processed(&source_str, mt, size, None, "error");
            }
            ProcessResult::Error { path: file.to_path_buf(), reason: e.to_string() }
        }
    }
}

fn print_summary(results: &[ProcessResult]) {
    let organized = results.iter().filter(|r| matches!(r, ProcessResult::Organized { .. })).count();
    let conflicts = results.iter().filter(|r| matches!(r, ProcessResult::ConflictResolved { .. })).count();
    let unsorted = results.iter().filter(|r| matches!(r, ProcessResult::Unsorted { .. })).count();
    let errors = results.iter().filter(|r| matches!(r, ProcessResult::Error { .. })).count();

    println!("\n{}", "Traitement terminé :".bold());
    if organized > 0 { println!("  {} {} fichiers organisés", "✓".green().bold(), organized); }
    if conflicts > 0 { println!("  {} {} conflits résolus (meilleur bitrate conservé)", "↑".cyan().bold(), conflicts); }
    if unsorted > 0 { println!("  {} {} fichiers non identifiés → _unsorted/", "⚠".yellow().bold(), unsorted); }
    if errors > 0 { println!("  {} {} erreurs", "✗".red().bold(), errors); }
}
