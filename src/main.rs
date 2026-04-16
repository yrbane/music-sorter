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

fn main() -> Result<()> {
    let args = cli::Args::parse();
    let config = config::Config::load()?;

    let source = args.source_path();
    let target = args.target_path();

    println!("{} {}", "Source:".bold(), source.display());
    println!("{} {}", "Destination:".bold(), target.display());

    if !source.exists() {
        eprintln!("{} Le dossier source n'existe pas : {}", "✗".red().bold(), source.display());
        std::process::exit(1);
    }

    let files = scanner::scan(&source);
    println!("\n{} fichiers audio trouvés\n", files.len().to_string().bold());

    if files.is_empty() {
        println!("Rien à faire.");
        return Ok(());
    }

    let enricher = Enricher::new(&config)?;

    let results: Vec<ProcessResult> = if args.workers > 1 {
        process_parallel(&files, &enricher, &target, args.r#move, args.workers)
    } else {
        process_sequential(&files, &enricher, &target, args.r#move)
    };

    print_summary(&results);
    Ok(())
}

fn process_sequential(
    files: &[std::path::PathBuf],
    enricher: &Enricher,
    target: &std::path::Path,
    do_move: bool,
) -> Vec<ProcessResult> {
    files.iter().map(|file| process_file(file, enricher, target, do_move)).collect()
}

fn process_parallel(
    files: &[std::path::PathBuf],
    enricher: &Enricher,
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
        files.par_iter().map(|file| process_file(file, enricher, target, do_move)).collect()
    })
}

fn process_file(
    file: &std::path::Path,
    enricher: &Enricher,
    target: &std::path::Path,
    do_move: bool,
) -> ProcessResult {
    let filename = file.file_name().unwrap_or_default().to_string_lossy();

    let info = match enricher.enrich(file) {
        Ok(info) => info,
        Err(e) => {
            eprintln!("  {} {} — {}", "✗".red().bold(), filename, e);
            return ProcessResult::Error { path: file.to_path_buf(), reason: e.to_string() };
        }
    };

    let dest = organizer::build_destination_path(target, &info, file);

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
                ProcessResult::Unsorted { from: file.to_path_buf(), to: dest }
            } else {
                ProcessResult::Organized { from: file.to_path_buf(), to: dest }
            }
        }
        Ok(organizer::CopyResult::Replaced { bitrate }) => {
            if let Err(e) = tags::write_tags(&dest, &info) {
                eprintln!("  ⚠ Erreur écriture tags après remplacement : {}", e);
            }
            println!("  {} {} — remplacé ({}kbps)", "↑".cyan().bold(), filename, bitrate);
            if do_move { let _ = organizer::remove_source(file); }
            ProcessResult::ConflictResolved { path: dest, kept_bitrate: bitrate }
        }
        Ok(organizer::CopyResult::Skipped { existing_bitrate }) => {
            println!("  {} {} — ignoré (existant : {}kbps)", "—".dimmed(), filename, existing_bitrate);
            ProcessResult::Organized { from: file.to_path_buf(), to: dest }
        }
        Err(e) => {
            eprintln!("  {} {} — {}", "✗".red().bold(), filename, e);
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
