mod album_group;
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
mod rollback;
mod scanner;
mod tags;
mod title_cleaner;

use crate::enricher::Enricher;
use crate::models::ProcessResult;
use anyhow::Result;
use clap::Parser;
use colored::*;
use indicatif::{ProgressBar, ProgressStyle};
use std::sync::Arc;
use std::time::Instant;

/// Options de run propagées à chaque worker (évite des signatures à rallonge).
struct RunOptions {
    do_move: bool,
    template: String,
    unsorted_ttl_days: i64,
}

fn main() -> Result<()> {
    let config = config::Config::load()?;
    let args = cli::Args::parse().resolve(&config);

    // Modes spéciaux qui n'ont pas besoin du dossier source
    if args.list_processed || args.rollback {
        // Le cache doit exister à l'emplacement de la target
        if !args.target.join(".music-sorter.db").exists() {
            eprintln!(
                "{} Pas de cache trouvé à {} (rien à lister/défaire)",
                "✗".red().bold(),
                args.target.display()
            );
            std::process::exit(1);
        }
        let cache = cache::Cache::open(&args.target)?;
        if args.list_processed {
            return rollback::print_list(&cache);
        }
        if args.rollback {
            rollback::run(&cache, args.apply)?;
            return Ok(());
        }
    }

    println!("{} {}", "Source:".bold(), args.source.display());
    println!("{} {}", "Destination:".bold(), args.target.display());

    if !args.source.exists() {
        eprintln!("{} Le dossier source n'existe pas : {}", "✗".red().bold(), args.source.display());
        std::process::exit(1);
    }

    // Création de la destination + ouverture du cache SQLite partagé
    let cache = if config.cache_enabled.unwrap_or(true) {
        std::fs::create_dir_all(&args.target)?;
        Arc::new(cache::Cache::open(&args.target)?)
    } else {
        println!("{}", "Cache désactivé (cache_enabled = false)".dimmed());
        Arc::new(cache::Cache::open_in_memory()?)
    };

    let files = scanner::scan(&args.source);
    println!("\n{} fichiers audio trouvés\n", files.len().to_string().bold());

    if files.is_empty() {
        println!("Rien à faire.");
        return Ok(());
    }

    let enricher = Enricher::new(&config, cache.clone())?;

    // Barre de progression : auto-désactivée si stdout n'est pas un TTY
    let bar = ProgressBar::new(files.len() as u64);
    bar.set_style(
        ProgressStyle::with_template(
            "  {spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {pos}/{len} ({per_sec}, ETA {eta})\n  {wide_msg}",
        )
        .unwrap()
        .progress_chars("█▉▊▋▌▍▎▏ "),
    );
    let bar = Arc::new(bar);

    let opts = Arc::new(RunOptions {
        do_move: args.do_move,
        template: config
            .naming_template
            .clone()
            .unwrap_or_else(|| organizer::DEFAULT_TEMPLATE.to_string()),
        unsorted_ttl_days: config.unsorted_ttl_days.unwrap_or(30),
    });

    let started = Instant::now();
    let results: Vec<ProcessResult> = if args.workers > 1 {
        process_parallel(
            &files,
            &enricher,
            &cache,
            &args.source,
            &args.target,
            &opts,
            args.workers,
            &bar,
        )
    } else {
        process_sequential(&files, &enricher, &cache, &args.source, &args.target, &opts, &bar)
    };
    let elapsed = started.elapsed();
    bar.finish_and_clear();

    print_summary(&results, elapsed);

    // Cleanup post-run : en mode --move, supprime les dossiers source devenus vides
    if args.do_move {
        let removed = organizer::cleanup_empty_dirs(&args.source);
        if removed > 0 {
            println!("  {} {} dossiers source vides nettoyés", "·".dimmed(), removed);
        }
    }
    Ok(())
}

fn process_sequential(
    files: &[std::path::PathBuf],
    enricher: &Enricher,
    cache: &Arc<cache::Cache>,
    source: &std::path::Path,
    target: &std::path::Path,
    opts: &Arc<RunOptions>,
    bar: &Arc<ProgressBar>,
) -> Vec<ProcessResult> {
    files
        .iter()
        .map(|file| process_file(file, enricher, cache, source, target, opts, bar))
        .collect()
}

fn process_parallel(
    files: &[std::path::PathBuf],
    enricher: &Enricher,
    cache: &Arc<cache::Cache>,
    source: &std::path::Path,
    target: &std::path::Path,
    opts: &Arc<RunOptions>,
    workers: usize,
    bar: &Arc<ProgressBar>,
) -> Vec<ProcessResult> {
    use rayon::prelude::*;

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(workers)
        .build()
        .expect("Impossible de créer le pool de threads");

    pool.install(|| {
        files
            .par_iter()
            .map(|file| process_file(file, enricher, cache, source, target, opts, bar))
            .collect()
    })
}

fn process_file(
    file: &std::path::Path,
    enricher: &Enricher,
    cache: &Arc<cache::Cache>,
    source: &std::path::Path,
    target: &std::path::Path,
    opts: &Arc<RunOptions>,
    bar: &Arc<ProgressBar>,
) -> ProcessResult {
    let filename = file.file_name().unwrap_or_default().to_string_lossy();
    bar.set_message(filename.to_string());
    // RAII guard : incrémente la barre dès qu'on quitte la fonction, quel que soit le chemin.
    struct Tick<'a>(&'a ProgressBar);
    impl Drop for Tick<'_> {
        fn drop(&mut self) {
            self.0.inc(1);
        }
    }
    let _tick = Tick(bar);

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

    // Skip total via cache : organized/conflict toujours skip ; unsorted skip seulement
    // pendant unsorted_ttl_days pour laisser une chance que MusicBrainz s'enrichisse.
    if let Some(mt) = mtime {
        if let Ok(Some((status, dest_opt, last_seen))) = cache.lookup_processed(&source_str, mt, size) {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let age_secs = now - last_seen;

            let should_skip = match status.as_str() {
                "organized" | "conflict" => true,
                "unsorted" => age_secs < opts.unsorted_ttl_days * 86400,
                _ => false,
            };

            if should_skip {
                if let Some(d) = dest_opt {
                    let dest_pb = std::path::PathBuf::from(d);
                    if dest_pb.exists() {
                        return ProcessResult::CachedSkip {
                            from: file.to_path_buf(),
                            to: dest_pb,
                        };
                    }
                }
            }
        }
    }

    // Catch des panics éventuels (bugs UTF-8 dans lofty, etc.) pour que le run continue
    let enrich_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        enricher.enrich(file)
    }));

    let info = match enrich_result {
        Ok(Ok(info)) => info,
        Ok(Err(e)) => {
            bar.println(format!("  {} {} — {}", "✗".red().bold(), filename, e));
            if let Some(mt) = mtime {
                let _ = cache.record_processed(&source_str, mt, size, None, "error");
            }
            return ProcessResult::Error { path: file.to_path_buf(), reason: e.to_string() };
        }
        Err(panic) => {
            let reason = panic
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "panic interne (UTF-8 ?)".into());
            bar.println(format!("  {} {} — PANIC : {}", "✗".red().bold(), filename, reason));
            if let Some(mt) = mtime {
                let _ = cache.record_processed(&source_str, mt, size, None, "error");
            }
            return ProcessResult::Error { path: file.to_path_buf(), reason };
        }
    };

    let dest = organizer::build_destination_path_with_template(
        target, &info, file, source, &opts.template,
    );

    // En mode --move : rename(2) atomique sur même FS, sinon copy + delete (cross-FS).
    // La source est consommée par move_to_destination dans tous les cas de succès.
    let copy_result = if opts.do_move {
        organizer::move_to_destination(file, &dest)
    } else {
        organizer::copy_to_destination(file, &dest)
    };

    match copy_result {
        Ok(organizer::CopyResult::Copied) => {
            if let Err(e) = tags::write_tags(&dest, &info) {
                bar.println(format!("  {} {} — Copié mais erreur tags : {}", "⚠".yellow().bold(), filename, e));
            }

            let is_unsorted = dest.to_string_lossy().contains("_unsorted");
            if is_unsorted {
                bar.println(format!("  {} {} → _unsorted/", "⚠".yellow().bold(), filename));
            }
            // Les succès ne s'affichent plus ligne par ligne : la barre + le résumé suffisent.

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
                bar.println(format!("  ⚠ Erreur écriture tags après remplacement : {}", e));
            }
            bar.println(format!("  {} {} — remplacé ({}kbps)", "↑".cyan().bold(), filename, bitrate));
            if let Some(mt) = mtime {
                let _ = cache.record_processed(&source_str, mt, size, Some(&dest.to_string_lossy()), "conflict");
            }
            ProcessResult::ConflictResolved { path: dest, kept_bitrate: bitrate }
        }
        Ok(organizer::CopyResult::Skipped { existing_bitrate }) => {
            bar.println(format!("  {} {} — ignoré (existant : {}kbps)", "—".dimmed(), filename, existing_bitrate));
            if let Some(mt) = mtime {
                let _ = cache.record_processed(&source_str, mt, size, Some(&dest.to_string_lossy()), "organized");
            }
            ProcessResult::Organized { from: file.to_path_buf(), to: dest }
        }
        Err(e) => {
            bar.println(format!("  {} {} — {}", "✗".red().bold(), filename, e));
            if let Some(mt) = mtime {
                let _ = cache.record_processed(&source_str, mt, size, None, "error");
            }
            ProcessResult::Error { path: file.to_path_buf(), reason: e.to_string() }
        }
    }
}

fn print_summary(results: &[ProcessResult], elapsed: std::time::Duration) {
    let organized = results.iter().filter(|r| matches!(r, ProcessResult::Organized { .. })).count();
    let cached = results.iter().filter(|r| matches!(r, ProcessResult::CachedSkip { .. })).count();
    let conflicts = results.iter().filter(|r| matches!(r, ProcessResult::ConflictResolved { .. })).count();
    let unsorted = results.iter().filter(|r| matches!(r, ProcessResult::Unsorted { .. })).count();
    let errors = results.iter().filter(|r| matches!(r, ProcessResult::Error { .. })).count();

    let total = results.len();
    let secs = elapsed.as_secs_f64().max(0.001);
    let throughput = total as f64 / secs;

    println!("\n{}", "Traitement terminé :".bold());
    if organized > 0 { println!("  {} {} fichiers organisés", "✓".green().bold(), organized); }
    if cached > 0    { println!("  {} {} ignorés depuis le cache (instantané)", "—".dimmed(), cached); }
    if conflicts > 0 { println!("  {} {} conflits résolus (meilleur bitrate conservé)", "↑".cyan().bold(), conflicts); }
    if unsorted > 0  { println!("  {} {} fichiers non identifiés → _unsorted/", "⚠".yellow().bold(), unsorted); }
    if errors > 0    { println!("  {} {} erreurs", "✗".red().bold(), errors); }
    println!(
        "  {} en {:.1}s ({:.1} fichiers/s)",
        "⏱".dimmed(),
        secs,
        throughput
    );
}
