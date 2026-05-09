use std::io::{self, BufReader, BufWriter};

use anyhow::{Context, Result};
use clap::Args;

use doubri_core::similarity::compute_pairwise_similarity;

#[derive(Args)]
pub struct SimilarityArgs {
    /// N-gram size
    #[arg(short = 'n', long, default_value_t = 5)]
    pub ngram: usize,

    /// Minimum similarity threshold
    #[arg(short = 's', long, default_value_t = 0.6)]
    pub threshold: f64,

    /// JSON ID field name
    #[arg(short = 'i', long = "id-field", default_value = "id")]
    pub id_field: String,

    /// JSON text field name
    #[arg(short = 't', long = "text-field", default_value = "text")]
    pub text_field: String,
}

pub fn run(args: SimilarityArgs) -> Result<()> {
    let stdout = io::stdout();
    let mut writer = BufWriter::new(stdout.lock());

    let count = {
        let stdin = io::stdin();
        let reader = BufReader::new(stdin.lock());
        compute_pairwise_similarity(
            reader,
            &mut writer,
            args.ngram,
            args.threshold,
            &args.id_field,
            &args.text_field,
        )
        .context("similarity computation failed")?
    };

    eprintln!("Found {} pairs above threshold {}", count, args.threshold);
    Ok(())
}
