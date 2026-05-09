use std::fs::File;
use std::io::{self, BufReader, BufWriter};

use anyhow::{Context, Result};
use clap::Args;

use doubri_core::apply::apply_each;
use doubri_core::format::{read_dup_flags, read_src_file};

#[derive(Args)]
pub struct ApplyEachArgs {
    /// Target MinHash file name (looked up in .src to compute offset)
    pub target: String,

    /// Path to the flag file (.dup or .dup.merge)
    #[arg(short = 'f', long)]
    pub flag: String,

    /// Source file list (.src file)
    #[arg(short = 's', long)]
    pub source: String,
}

pub fn run(args: ApplyEachArgs) -> Result<()> {
    let flags = read_dup_flags(&mut File::open(&args.flag)?)
        .with_context(|| format!("failed to read flag file {}", args.flag))?;

    let src_entries = read_src_file(&mut File::open(&args.source)?)
        .with_context(|| format!("failed to read source file {}", args.source))?;

    // Compute target offset
    let mut offset = 0usize;
    let mut found = false;
    for entry in &src_entries {
        if entry.file_path == args.target || entry.file_path.ends_with(&args.target) {
            found = true;
            break;
        }
        offset += entry.item_count as usize;
    }

    if !found {
        anyhow::bail!(
            "target '{}' not found in source file '{}'",
            args.target,
            args.source
        );
    }

    let stdout = io::stdout();
    let mut writer = BufWriter::new(stdout.lock());

    let count = {
        let stdin = io::stdin();
        let reader = BufReader::new(stdin.lock());
        apply_each(reader, &mut writer, &flags, offset).context("apply-each failed")?
    };

    eprintln!("Output {} documents", count);
    Ok(())
}
