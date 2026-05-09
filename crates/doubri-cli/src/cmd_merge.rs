use std::fs::File;

use anyhow::{Context, Result};
use clap::Args;

use doubri_core::format::read_dup_flags;
use doubri_core::merge::{MergeSource, merge_groups};

#[derive(Args)]
pub struct MergeArgs {
    /// Source base names (multiple allowed)
    pub sources: Vec<String>,

    /// Keep the newer document
    #[arg(short = 'r', long)]
    pub reverse: bool,

    /// Start split index
    #[arg(short = 's', long, default_value_t = 0)]
    pub start: u8,

    /// End split index
    #[arg(short = 'e', long, default_value_t = 255)]
    pub end: u8,
}

pub fn run(args: MergeArgs) -> Result<()> {
    if args.sources.is_empty() {
        anyhow::bail!("no source basenames provided");
    }

    let mut sources = Vec::new();
    for basename in &args.sources {
        let dup_path = format!("{}.dup", basename);
        let flags = read_dup_flags(&mut File::open(&dup_path)?)
            .with_context(|| format!("failed to read {}", dup_path))?;
        sources.push(MergeSource {
            basename: basename.clone(),
            doc_count: flags.len(),
        });
    }

    let result =
        merge_groups(&sources, args.start, args.end, args.reverse).context("merge failed")?;

    eprintln!("Cross-group duplicates: {}", result.cross_duplicates);
    Ok(())
}
