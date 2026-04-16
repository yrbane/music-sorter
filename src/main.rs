mod cli;
mod config;
mod models;

use clap::Parser;

fn main() {
    let args = cli::Args::parse();
    println!("Source: {}", args.source);
    println!("Destination: {}", args.target);
    println!("Workers: {}", args.workers);
    println!("Move: {}", args.r#move);
}
