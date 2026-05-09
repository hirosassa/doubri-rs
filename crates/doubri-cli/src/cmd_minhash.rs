use std::fs::File;
use std::io::{self, BufReader, BufWriter};

use anyhow::{Context, Result};
use clap::Args;

use doubri_core::minhash::{MinHashConfig, process_jsonl};

#[derive(Args)]
pub struct MinhashArgs {
    /// Output file name
    pub output: String,

    /// N-gram size
    #[arg(short = 'n', long, default_value_t = 5)]
    pub ngram: usize,

    /// Number of hash values per bucket
    #[arg(short = 'b', long = "band-size", default_value_t = 20)]
    pub band_size: usize,

    /// Number of buckets
    #[arg(short = 'r', long = "num-buckets", default_value_t = 40)]
    pub num_buckets: usize,

    /// JSON text field name
    #[arg(short = 't', long = "text-field", default_value = "text")]
    pub text_field: String,
}

pub fn run(args: MinhashArgs) -> Result<()> {
    let config = MinHashConfig {
        ngram_size: args.ngram,
        num_buckets: args.num_buckets,
        band_size: args.band_size,
    };

    let mut writer = BufWriter::new(
        File::create(&args.output).with_context(|| format!("failed to create {}", args.output))?,
    );

    let count = {
        let stdin = io::stdin();
        let reader = BufReader::new(stdin.lock());
        process_jsonl(reader, &mut writer, &config, &args.text_field).context("minhash failed")?
    };

    eprintln!("Processed {} documents -> {}", count, args.output);
    Ok(())
}
