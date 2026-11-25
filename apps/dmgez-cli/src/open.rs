use std::path::{Path, PathBuf};

use clap::Args;

use crate::CliResult;

pub(super) const FEATURE: &str = "desktop-open";

#[derive(Args)]
pub(super) struct Arguments {
	/// Local TOML project or DMG image.
	document: PathBuf,
	/// Path to the installed dmgez-gui executable. Defaults to a neighbor of dmgez.
	#[arg(long, value_name = "EXECUTABLE")]
	gui: Option<PathBuf>,
}

pub(super) fn run(arguments: &Arguments) -> CliResult {
	launch(&arguments.document, arguments.gui.as_deref())
}

#[cfg(feature = "desktop-open")]
fn launch(document: &Path, gui: Option<&Path>) -> CliResult {
	dmgez_runtime::desktop::launch(document, gui)?;
	Ok(())
}

#[cfg(not(feature = "desktop-open"))]
fn launch(_: &Path, _: Option<&Path>) -> CliResult {
	Err(format!("feature not compiled: {FEATURE}").into())
}
