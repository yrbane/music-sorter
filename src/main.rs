mod cli;
mod config;
mod models;
mod scanner;

use clap::Parser;

fn main() {
    let args = cli::Args::parse();
    let files = scanner::scan(&args.source_path());
    println!("{} fichiers audio trouvés", files.len());
    for f in &files {
        println!("  {}", f.display());
    }
}
