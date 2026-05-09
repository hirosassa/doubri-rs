mod cmd_apply_each;
mod cmd_apply_whole;
mod cmd_dedup;
mod cmd_merge;
mod cmd_minhash;
mod cmd_similarity;

use clap::{Parser, Subcommand};

/// doubri - Large-scale document deduplication tool
#[derive(Parser)]
#[command(name = "doubri", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Convert JSONL to MinHash bucket files
    Minhash(cmd_minhash::MinhashArgs),

    /// Within-group deduplication
    Dedup(cmd_dedup::DedupArgs),

    /// Cross-group deduplication (index merge)
    Merge(cmd_merge::MergeArgs),

    /// Jaccard similarity computation (for verification)
    Similarity(cmd_similarity::SimilarityArgs),

    /// Output non-duplicate documents at once based on flags
    ApplyWhole(cmd_apply_whole::ApplyWholeArgs),

    /// Output non-duplicate documents per source based on flags
    ApplyEach(cmd_apply_each::ApplyEachArgs),
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Minhash(args) => cmd_minhash::run(args),
        Commands::Dedup(args) => cmd_dedup::run(args),
        Commands::Merge(args) => cmd_merge::run(args),
        Commands::Similarity(args) => cmd_similarity::run(args),
        Commands::ApplyWhole(args) => cmd_apply_whole::run(args),
        Commands::ApplyEach(args) => cmd_apply_each::run(args),
    }
}
