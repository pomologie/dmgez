use std::path::PathBuf;

use clap::Args;

use crate::CliResult;

pub(super) const FEATURE: &str = "project-edit";
const DEFINITION_VALUE: &str = "NAME=VALUE";
const SHA256_VALUE: &str = "HEX";

#[derive(Args)]
pub(super) struct Arguments {
	/// Existing TOML project. Its contents and image stay unchanged.
	project: PathBuf,
	/// TOML file containing the ordered typed edit batch.
	#[arg(long)]
	changes: PathBuf,
	/// New image path in project syntax. Relative paths use the project directory.
	#[arg(short, long)]
	output: String,
	/// Require the original project to match this SHA-256 before planning.
	#[arg(long, value_name = SHA256_VALUE)]
	expect_project_sha256: Option<String>,
	/// Define a literal project substitution. May be repeated.
	#[arg(long, value_name = DEFINITION_VALUE)]
	define: Vec<String>,
	/// Plan and seal the rebuild without creating a new file.
	#[arg(long)]
	dry_run: bool,
	/// Write a versioned JSON report.
	#[arg(long)]
	json: bool,
}

#[cfg(feature = "project-edit")]
mod execute;

#[cfg(feature = "project-edit")]
pub(super) fn run(arguments: &Arguments) -> CliResult {
	execute::run(arguments)
}

#[cfg(not(feature = "project-edit"))]
pub(super) fn run(_arguments: &Arguments) -> CliResult {
	Err(format!("project editing requires {FEATURE} in an HFS+ composed CLI").into())
}
