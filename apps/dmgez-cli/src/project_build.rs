use std::path::PathBuf;

use clap::Args;

#[cfg(feature = "project-build")]
mod execute;
#[cfg(feature = "project-release")]
mod release;
#[cfg(feature = "project-release")]
mod release_artifact;
#[cfg(feature = "project-release")]
mod release_native;
#[cfg(feature = "project-release")]
mod release_settings;

#[cfg(feature = "project-build")]
pub(crate) use execute::build;

const DEFINE_VALUE_NAME: &str = "NAME=VALUE";
const OVERRIDE_VALUE_NAME: &str = "SECTION.FIELD=VALUE";
const RELEASE_ARG: &str = "release";

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
	/// Execute the explicitly configured release stages on a staged image.
	#[arg(long, conflicts_with = "overwrite")]
	release: bool,
	/// Code-signing identifier for a signed disk image.
	#[arg(long, requires = RELEASE_ARG)]
	identifier: Option<String>,
	/// New local notarization record for this submission.
	#[arg(long, requires = RELEASE_ARG)]
	notary_record: Option<PathBuf>,
	/// Emit the release stage report as JSON.
	#[arg(long, requires = RELEASE_ARG, conflicts_with = "dry_run")]
	json: bool,
}
