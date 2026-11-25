use std::path::PathBuf;

use clap::Args;

#[cfg(feature = "project-build")]
mod execute;

#[cfg(feature = "project-build")]
pub(crate) use execute::build;

const DEFINE_VALUE_NAME: &str = "NAME=VALUE";
const OVERRIDE_VALUE_NAME: &str = "SECTION.FIELD=VALUE";

#[derive(Args)]
pub(crate) struct Arguments {
	/// TOML project file. Relative paths use the caller's working directory.
	project: PathBuf,
	/// Plan the exact virtual disk capacity and report the resolved build summary.
	#[arg(long)]
	dry_run: bool,
	/// Define a literal substitution value. May be repeated.
	#[arg(long, value_name = DEFINE_VALUE_NAME)]
	define: Vec<String>,
	/// Override a typed scalar project setting. May be repeated.
	#[arg(long, value_name = OVERRIDE_VALUE_NAME)]
	set: Vec<String>,
	/// Authorize staged replacement in a trusted local output directory.
	#[arg(long)]
	overwrite: bool,
}
