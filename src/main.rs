mod cli;
mod config;
mod coverart;
mod discogs;
mod enricher;
mod fingerprint;
mod models;
mod musicbrainz;
mod rate_limiter;
mod organizer;
mod scanner;
mod tags;

use clap::Parser;

fn main() {
    let args = cli::Args::parse();
    let files = scanner::scan(&args.source_path());
    println!("{} fichiers audio trouvés", files.len());
    for f in &files {
        println!("  {}", f.display());
    }
}
