use std::io::{self, BufReader};

use anyhow::{Context, Result};
use clap::Args;

use doubri_core::dedup::{dedup_group, read_hash_file_list};

#[derive(Args)]
pub struct DedupArgs {
    /// Output file base name
    pub basename: String,

    /// Keep the newer document
    #[arg(short = 'r', long)]
    pub reverse: bool,
}

pub fn run(args: DedupArgs) -> Result<()> {
    let paths = {
        let stdin = io::stdin();
        let reader = BufReader::new(stdin.lock());
        read_hash_file_list(reader).context("failed to read hash file list")?
    };

    if paths.is_empty() {
        anyhow::bail!("no hash files provided on stdin");
    }

    let result =
        dedup_group(&paths, &args.basename, args.reverse).context("deduplication failed")?;

    eprintln!(
        "Total: {} documents, {} duplicates ({:.1}%)",
        result.total_documents,
        result.duplicate_count,
        result.duplicate_count as f64 / result.total_documents as f64 * 100.0
    );
    Ok(())
}
