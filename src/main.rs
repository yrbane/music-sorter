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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

/// Options de run propagées à chaque worker (évite des signatures à rallonge).
struct RunOptions {
    do_move: bool,
    dry_run: bool,
    resume: bool,
    retry_unsorted: bool,
    dedup: bool,
    audio_dedup: bool,
    quarantine: bool,
    fix_tags: bool,
    template: String,
    unsorted_ttl_days: i64,
    /// Passé à true par le handler Ctrl-C : les workers restants s'arrêtent net.
    interrupted: Arc<AtomicBool>,
}

fn main() -> Result<()> {
    let config = config::Config::load()?;
    let args = cli::Args::parse().resolve(&config);

    // Modes spéciaux qui n'ont pas besoin du dossier source
    if args.list_processed || args.list_unsorted || args.rollback {
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
        if args.list_unsorted {
            return rollback::print_unsorted(&cache);
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

    if args.dry_run {
        println!("{}", "Mode DRY-RUN : aucun fichier ne sera copié/déplacé.".yellow().bold());
    }

    // Handler Ctrl-C : bascule le flag partagé. Le cache WAL persiste chaque fichier
    // déjà traité, donc une relance reprend automatiquement là où on s'est arrêté.
    let interrupted = Arc::new(AtomicBool::new(false));
    {
        let flag = interrupted.clone();
        let _ = ctrlc::set_handler(move || {
            flag.store(true, Ordering::SeqCst);
            eprintln!("\n{}", "Interruption demandée — arrêt après le fichier en cours…".yellow().bold());
        });
    }

    let opts = Arc::new(RunOptions {
        do_move: args.do_move,
        dry_run: args.dry_run,
        resume: args.resume,
        retry_unsorted: args.retry_unsorted,
        dedup: config.dedup_enabled.unwrap_or(true),
        // Dédup acoustique : nécessite fpcalc ; désactivable via config audio_dedup.
        audio_dedup: config.audio_dedup.unwrap_or(true) && fingerprint::is_fpcalc_available(),
        quarantine: config.quarantine_enabled.unwrap_or(true),
        fix_tags: args.fix_tags || config.fix_tags.unwrap_or(false),
        template: config
            .naming_template
            .clone()
            .unwrap_or_else(|| organizer::DEFAULT_TEMPLATE.to_string()),
        unsorted_ttl_days: config.unsorted_ttl_days.unwrap_or(30),
        interrupted,
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
    if args.do_move && !args.dry_run {
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
    // Interruption (Ctrl-C) : on arrête net sans traiter ni enregistrer.
    if opts.interrupted.load(Ordering::SeqCst) {
        return ProcessResult::Interrupted { path: file.to_path_buf() };
    }

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

            if should_skip_cached(&status, age_secs, opts.unsorted_ttl_days, opts.resume, opts.retry_unsorted) {
                if let Some(d) = dest_opt {
                    let dest_pb = std::path::PathBuf::from(d);
                    if dest_pb.exists() {
                        // Copie source déjà organisée (ancien run en mode copie) :
                        // en --move on la met à la corbeille pour vider la source.
                        if should_trash_redundant_source(&status, opts.do_move, true) {
                            let _ = trash::delete(file);
                        }
                        return ProcessResult::CachedSkip {
                            from: file.to_path_buf(),
                            to: dest_pb,
                        };
                    }
                }
            }
        }
    }

    // Détection de doublons par hash de contenu : si un fichier au contenu identique
    // a déjà été rangé ailleurs, on l'ignore (et on consomme la source en mode --move).
    // Calculé avant l'enrichissement pour économiser aussi les appels API.
    let content_hash: Option<String> = if opts.dedup && !opts.dry_run {
        match cache_keys::content_hash(file) {
            Ok(h) => {
                if let Ok(Some(existing)) = cache.lookup_content(&h) {
                    if std::path::Path::new(&existing).exists() {
                        bar.println(format!("  {} {} — doublon de {}", "⧉".cyan().bold(), filename, existing));
                        let of = std::path::PathBuf::from(&existing);
                        if opts.do_move {
                            let _ = std::fs::remove_file(file);
                        }
                        if let Some(mt) = mtime {
                            let _ = cache.record_processed(&source_str, mt, size, Some(&existing), "organized");
                        }
                        return ProcessResult::Duplicate { from: file.to_path_buf(), of };
                    }
                }
                Some(h)
            }
            Err(_) => None,
        }
    } else {
        None
    };

    // Dédup acoustique : deux fichiers renvoyant le même enregistrement AcoustID
    // (MBID) sont le même morceau, quelle que soit la qualité. On conserve le
    // meilleur bitrate, l'autre part à la corbeille système.
    let acoustic: Option<(String, u32)> = if opts.audio_dedup && !opts.dry_run {
        match enricher.resolve_recording_id(file) {
            Some(recording_id) => {
                let cur_q = tags::get_bitrate(file).unwrap_or(0);
                if let Ok(Some((existing, existing_q))) = cache.lookup_acoustic(&recording_id) {
                    let existing_path = std::path::PathBuf::from(&existing);
                    let same_file =
                        existing_path.canonicalize().ok() == file.canonicalize().ok();
                    if existing_path.exists() && !same_file {
                        if cur_q > existing_q {
                            // Courant meilleur : l'ancien exemplaire va à la corbeille.
                            let _ = trash::delete(&existing_path);
                            bar.println(format!(
                                "  {} {} — remplace un doublon acoustique ({}→{}kbps)",
                                "⧉".cyan().bold(), filename, existing_q, cur_q
                            ));
                        } else {
                            // Ancien au moins aussi bon : le courant est le perdant.
                            if opts.do_move {
                                let _ = trash::delete(file);
                            }
                            bar.println(format!(
                                "  {} {} — doublon acoustique (corbeille, {}≤{}kbps)",
                                "⧉".cyan().bold(), filename, cur_q, existing_q
                            ));
                            if let Some(mt) = mtime {
                                let _ = cache.record_processed(&source_str, mt, size, Some(&existing), "organized");
                            }
                            return ProcessResult::AcousticDuplicate {
                                from: file.to_path_buf(),
                                of: existing_path,
                            };
                        }
                    }
                }
                Some((recording_id, cur_q))
            }
            None => None,
        }
    } else {
        None
    };

    // Catch des panics éventuels (bugs UTF-8 dans lofty, etc.) pour que le run continue
    let enrich_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        enricher.enrich(file)
    }));

    let (info, confidence) = match enrich_result {
        Ok(Ok(pair)) => pair,
        Ok(Err(e)) => {
            // Erreur d'enrichissement (JSON API malformé, DB…) : transitoire →
            // on laisse le fichier en source pour re-tentative au prochain run.
            bar.println(format!("  {} {} — {}", "✗".red().bold(), filename, e));
            if let Some(mt) = mtime {
                let _ = cache.record_processed_note(&source_str, mt, size, None, "error", Some(&e.to_string()));
            }
            return ProcessResult::Error { path: file.to_path_buf(), reason: e.to_string() };
        }
        Err(panic) => {
            let reason = panic
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "panic interne (UTF-8 ?)".into());
            bar.println(format!("  {} {} — PANIC : {} → _errors/", "✗".red().bold(), filename, reason));
            let quarantined = quarantine_error(file, target, source, opts.do_move);
            let dest_str = quarantined.as_ref().map(|d| d.to_string_lossy().into_owned());
            if let Some(mt) = mtime {
                let _ = cache.record_processed_note(&source_str, mt, size, dest_str.as_deref(), "error", Some(&reason));
            }
            return ProcessResult::Error { path: file.to_path_buf(), reason };
        }
    };

    let mut dest = organizer::build_destination_path_with_template(
        target, &info, file, source, &opts.template,
    );

    // Quarantaine : un match de faible confiance (heuristique seule) part en _review/
    // au lieu de polluer l'arborescence principale. Les _unsorted restent inchangés.
    let is_unsorted_dest = dest.to_string_lossy().contains("_unsorted");
    if opts.quarantine
        && confidence == crate::models::Confidence::Low
        && !is_unsorted_dest
    {
        dest = organizer::redirect_to_review(target, &dest);
    }

    // En mode --fix-tags, on n'écrase les tags que sur un match sûr (High).
    let overwrite_tags = opts.fix_tags && confidence == crate::models::Confidence::High;

    // Mode dry-run : on a tout calculé (y compris l'enrichissement API), mais on
    // n'écrit rien sur le disque et on ne touche pas au cache processed_files.
    if opts.dry_run {
        let is_unsorted = dest.to_string_lossy().contains("_unsorted");
        if is_unsorted {
            bar.println(format!("  {} {} → _unsorted/", "⚠".yellow().bold(), filename));
            return ProcessResult::Unsorted { from: file.to_path_buf(), to: dest };
        }
        bar.println(format!("  {} {} → {}", "→".cyan().bold(), filename, dest.display()));
        return ProcessResult::Organized { from: file.to_path_buf(), to: dest };
    }

    // En mode --move : rename(2) atomique sur même FS, sinon copy + delete (cross-FS).
    // La source est consommée par move_to_destination dans tous les cas de succès.
    let copy_result = if opts.do_move {
        organizer::move_to_destination(file, &dest)
    } else {
        organizer::copy_to_destination(file, &dest)
    };

    match copy_result {
        Ok(organizer::CopyResult::Copied) => {
            if let Err(e) = tags::write_tags(&dest, &info, overwrite_tags) {
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
                if let Some(h) = &content_hash {
                    let _ = cache.record_content(h, &dest.to_string_lossy());
                }
                if let Some((fp_key, q)) = &acoustic {
                    let _ = cache.upsert_acoustic(fp_key, &dest.to_string_lossy(), *q);
                }
                ProcessResult::Organized { from: file.to_path_buf(), to: dest }
            }
        }
        Ok(organizer::CopyResult::Replaced { bitrate }) => {
            if let Err(e) = tags::write_tags(&dest, &info, overwrite_tags) {
                bar.println(format!("  ⚠ Erreur écriture tags après remplacement : {}", e));
            }
            bar.println(format!("  {} {} — remplacé ({}kbps)", "↑".cyan().bold(), filename, bitrate));
            if let Some(mt) = mtime {
                let _ = cache.record_processed(&source_str, mt, size, Some(&dest.to_string_lossy()), "conflict");
            }
            if let Some(h) = &content_hash {
                let _ = cache.record_content(h, &dest.to_string_lossy());
            }
            if let Some((fp_key, q)) = &acoustic {
                let _ = cache.upsert_acoustic(fp_key, &dest.to_string_lossy(), *q);
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
            // Erreur d'I/O au rangement (nom de destination invalide sur le FS,
            // chemin trop long…). On tente de mettre le fichier en quarantaine
            // sous _errors/ (nom d'origine, toujours valide). Si ce déplacement
            // échoue AUSSI (cible read-only, disque plein → vraiment
            // environnemental), on laisse en source pour re-tentative.
            let quarantined = quarantine_error(file, target, source, opts.do_move);
            let dest_str = quarantined.as_ref().map(|d| d.to_string_lossy().into_owned());
            let suffix = if quarantined.is_some() { " → _errors/" } else { "" };
            bar.println(format!("  {} {} — {}{}", "✗".red().bold(), filename, e, suffix));
            if let Some(mt) = mtime {
                let _ = cache.record_processed_note(&source_str, mt, size, dest_str.as_deref(), "error", Some(&e.to_string()));
            }
            ProcessResult::Error { path: file.to_path_buf(), reason: e.to_string() }
        }
    }
}

/// Décide si une entrée déjà vue en cache doit être skippée sans retraitement.
/// - organized/conflict : toujours skip (le fichier est rangé).
/// - unsorted : skip pendant `ttl_days`, ou toujours en mode `resume`.
/// - autre (error, ...) : jamais skip (on retente).
/// En mode `--move`, une source cachée « organized »/« conflict » dont la
/// destination existe déjà est une **copie redondante** (issue d'un ancien run en
/// mode copie) : elle doit partir à la corbeille pour que la source se vide.
/// (Le hit cache exige source_path+mtime+size identiques → c'est bien le même fichier.)
fn should_trash_redundant_source(status: &str, do_move: bool, dest_exists: bool) -> bool {
    do_move && dest_exists && matches!(status, "organized" | "conflict")
}

/// Déplace un fichier en erreur **de contenu** (illisible, tags corrompus) vers
/// `target/_errors/`, uniquement en mode `--move` (en copie la source n'est pas
/// consommée). Renvoie la destination si le déplacement a réussi. Les erreurs
/// d'I/O (copie/déplacement) ne passent pas par ici : le fichier reste en source
/// pour être re-tenté au prochain run.
fn quarantine_error(
    file: &std::path::Path,
    target: &std::path::Path,
    source: &std::path::Path,
    do_move: bool,
) -> Option<std::path::PathBuf> {
    if !do_move {
        return None;
    }
    let dest = organizer::error_destination(target, file, source);
    organizer::move_to_destination(file, &dest).ok().map(|_| dest)
}

fn should_skip_cached(
    status: &str,
    age_secs: i64,
    ttl_days: i64,
    resume: bool,
    retry_unsorted: bool,
) -> bool {
    match status {
        "organized" | "conflict" => true,
        // --retry-unsorted force la re-tentative (prime sur resume et le TTL).
        "unsorted" => !retry_unsorted && (resume || age_secs < ttl_days * 86400),
        _ => false,
    }
}

fn print_summary(results: &[ProcessResult], elapsed: std::time::Duration) {
    let organized = results.iter().filter(|r| matches!(r, ProcessResult::Organized { .. })).count();
    let cached = results.iter().filter(|r| matches!(r, ProcessResult::CachedSkip { .. })).count();
    let conflicts = results.iter().filter(|r| matches!(r, ProcessResult::ConflictResolved { .. })).count();
    let unsorted = results.iter().filter(|r| matches!(r, ProcessResult::Unsorted { .. })).count();
    let duplicates = results.iter().filter(|r| matches!(r, ProcessResult::Duplicate { .. })).count();
    let acoustic_dups = results.iter().filter(|r| matches!(r, ProcessResult::AcousticDuplicate { .. })).count();
    let interrupted = results.iter().filter(|r| matches!(r, ProcessResult::Interrupted { .. })).count();
    let errors = results.iter().filter(|r| matches!(r, ProcessResult::Error { .. })).count();

    let total = results.len();
    let secs = elapsed.as_secs_f64().max(0.001);
    let throughput = total as f64 / secs;

    println!("\n{}", "Traitement terminé :".bold());
    if organized > 0 { println!("  {} {} fichiers organisés", "✓".green().bold(), organized); }
    if cached > 0    { println!("  {} {} ignorés depuis le cache (instantané)", "—".dimmed(), cached); }
    if conflicts > 0 { println!("  {} {} conflits résolus (meilleur bitrate conservé)", "↑".cyan().bold(), conflicts); }
    if unsorted > 0  { println!("  {} {} fichiers non identifiés → _unsorted/", "⚠".yellow().bold(), unsorted); }
    if duplicates > 0 { println!("  {} {} doublons de contenu ignorés", "⧉".cyan().bold(), duplicates); }
    if acoustic_dups > 0 { println!("  {} {} doublons acoustiques → corbeille (meilleure qualité conservée)", "⧉".cyan().bold(), acoustic_dups); }
    if interrupted > 0 { println!("  {} {} non traités (interruption)", "⏹".yellow().bold(), interrupted); }
    if errors > 0    { println!("  {} {} erreurs → _errors/ (ou source si cible non inscriptible)", "✗".red().bold(), errors); }
    println!(
        "  {} en {:.1}s ({:.1} fichiers/s)",
        "⏱".dimmed(),
        secs,
        throughput
    );
}

#[cfg(test)]
mod tests {
    use super::{should_skip_cached, should_trash_redundant_source};

    #[test]
    fn test_trash_redundant_source_decision() {
        // --move + déjà organisée + dest existante → corbeille.
        assert!(should_trash_redundant_source("organized", true, true));
        assert!(should_trash_redundant_source("conflict", true, true));
        // Mode copie : on ne touche pas à la source.
        assert!(!should_trash_redundant_source("organized", false, true));
        // Dest absente : ce n'est pas une copie redondante.
        assert!(!should_trash_redundant_source("organized", true, false));
        // unsorted : laissé en source (peut vouloir une re-tentative).
        assert!(!should_trash_redundant_source("unsorted", true, true));
    }

    #[test]
    fn test_skip_organized_always() {
        assert!(should_skip_cached("organized", 999_999_999, 30, false, false));
        assert!(should_skip_cached("conflict", 999_999_999, 30, false, false));
    }

    #[test]
    fn test_skip_unsorted_within_ttl() {
        // 10 jours < 30 jours → skip
        assert!(should_skip_cached("unsorted", 10 * 86400, 30, false, false));
    }

    #[test]
    fn test_no_skip_unsorted_past_ttl() {
        // 40 jours > 30 jours → on retente
        assert!(!should_skip_cached("unsorted", 40 * 86400, 30, false, false));
    }

    #[test]
    fn test_resume_forces_skip_unsorted_past_ttl() {
        // En mode resume, on skip même au-delà du TTL
        assert!(should_skip_cached("unsorted", 40 * 86400, 30, true, false));
    }

    #[test]
    fn test_retry_unsorted_forces_reprocess() {
        // --retry-unsorted force la re-tentative, même en resume et dans le TTL.
        assert!(!should_skip_cached("unsorted", 10 * 86400, 30, true, true));
        assert!(!should_skip_cached("unsorted", 5 * 86400, 30, false, true));
    }

    #[test]
    fn test_no_skip_error() {
        assert!(!should_skip_cached("error", 0, 30, false, false));
        assert!(!should_skip_cached("error", 0, 30, true, false));
    }
}
