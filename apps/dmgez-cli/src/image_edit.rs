use std::path::PathBuf;

use clap::Args;

use crate::CliResult;

pub(super) const FEATURE: &str = "image-edit";
const SHA256_VALUE: &str = "HEX";

#[derive(Args)]
pub(super) struct Arguments {
	/// Existing partitionless HFS+ disk image.
	image: PathBuf,
	/// TOML file containing the typed edit batch.
	#[arg(long)]
	changes: PathBuf,
	/// Absent destination for the new unsigned image.
	#[arg(short, long)]
	output: PathBuf,
	/// Require the original image to match this full-file SHA-256.
	#[arg(long, value_name = SHA256_VALUE)]
	expect_image_sha256: Option<String>,
	/// Plan the rebuild without creating a stage or publishing an image.
	#[arg(long)]
	dry_run: bool,
	/// Write a versioned JSON report.
	#[arg(long)]
	json: bool,
}

#[cfg(feature = "image-edit")]
mod execute;

#[cfg(feature = "image-edit")]
pub(super) fn run(arguments: &Arguments) -> CliResult {
	execute::run(arguments)
}

#[cfg(not(feature = "image-edit"))]
pub(super) fn run(_arguments: &Arguments) -> CliResult {
	Err(format!("editing an image requires {FEATURE} in an HFS+ composed CLI").into())
}
