use std::fs::File;
use std::io::{self, BufReader, BufWriter};

use anyhow::{Context, Result};
use clap::Args;

use doubri_core::apply::apply_whole;
use doubri_core::format::read_dup_flags;

#[derive(Args)]
pub struct ApplyWholeArgs {
    /// Path to the flag file (.dup or .dup.merge)
    #[arg(short = 'f', long)]
    pub flag: String,
}

pub fn run(args: ApplyWholeArgs) -> Result<()> {
    let flags = read_dup_flags(&mut File::open(&args.flag)?)
        .with_context(|| format!("failed to read flag file {}", args.flag))?;

    let stdout = io::stdout();
    let mut writer = BufWriter::new(stdout.lock());

    let count = {
        let stdin = io::stdin();
        let reader = BufReader::new(stdin.lock());
        apply_whole(reader, &mut writer, &flags).context("apply-whole failed")?
    };

    eprintln!("Output {} documents", count);
    Ok(())
}
