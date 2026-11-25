use std::{
	num::NonZeroU32,
	path::{Path, PathBuf},
};

use clap::Args;

use crate::CliResult;

pub(super) const FEATURE: &str = "preview";
pub(super) const ACTUAL_FEATURE: &str = "actual-preview";
const ROOT_FOLDER: &str = "/";
const IMAGE_ARG: &str = "image";
const PROJECT_ARG: &str = "project";
const RAW_ARG: &str = "raw";
const PARTITION_ARG: &str = "partition";

#[cfg(feature = "actual-preview")]
mod actual;

#[derive(Args)]
pub(super) struct Arguments {
	/// Actual UDIF image to read without mounting it.
	#[arg(id = IMAGE_ARG, value_name = "IMAGE", required_unless_present = PROJECT_ARG)]
	image: Option<PathBuf>,
	/// TOML project whose declared design will be rendered.
	#[arg(long, id = PROJECT_ARG, conflicts_with_all = [IMAGE_ARG, RAW_ARG, PARTITION_ARG], required_unless_present = IMAGE_ARG)]
	project: Option<PathBuf>,
	/// New path for the PNG. Existing files are preserved.
	#[arg(short, long)]
	output: PathBuf,
	/// Virtual folder to render, defaults to the image root.
	#[arg(long, default_value = ROOT_FOLDER)]
	folder: String,
	/// Show entries marked hidden in the project or image.
	#[arg(long)]
	show_hidden: bool,
	/// Read the actual image as raw disk bytes instead of UDIF.
	#[arg(long, id = RAW_ARG, requires = IMAGE_ARG)]
	raw: bool,
	/// Select an actual image's one-based partition slot.
	#[arg(long, id = PARTITION_ARG, requires = IMAGE_ARG, value_name = crate::volume_commands::PARTITION_SLOT_VALUE_NAME)]
	partition: Option<NonZeroU32>,
}

pub(super) fn run(arguments: &Arguments) -> CliResult {
	match (arguments.project.as_deref(), arguments.image.as_deref()) {
		(Some(path), None) => design(path, arguments),
		(None, Some(path)) => image(path, arguments),
		_ => Err("select one project or one disk image".into()),
	}
}

#[cfg(feature = "preview")]
fn design(path: &Path, arguments: &Arguments) -> CliResult {
	use dmgez_runtime::preview::design_png;

	const DESIGN_MODE: &str = "design";

	let size = design_png(
		path,
		&arguments.folder.parse()?,
		arguments.visibility(),
		&arguments.output,
	)?;
	print_report(DESIGN_MODE, size);
	Ok(())
}

#[cfg(not(feature = "preview"))]
fn design(_: &Path, _: &Arguments) -> CliResult {
	Err(format!("feature not compiled: {FEATURE}").into())
}

#[cfg(feature = "actual-preview")]
use actual::image;

#[cfg(not(feature = "actual-preview"))]
fn image(_: &Path, _: &Arguments) -> CliResult {
	Err(format!("feature not compiled: {ACTUAL_FEATURE}").into())
}

#[cfg(feature = "preview")]
impl Arguments {
	fn visibility(&self) -> dmgez_runtime::preview::ItemVisibility {
		use dmgez_runtime::preview::ItemVisibility;

		if self.show_hidden {
			ItemVisibility::IncludeHidden
		} else {
			ItemVisibility::Visible
		}
	}
}

#[cfg(feature = "preview")]
fn print_report(mode: &str, (width, height): (u32, u32)) {
	const MODE_KEY: &str = "mode";
	const WIDTH_KEY: &str = "width";
	const HEIGHT_KEY: &str = "height";

	println!("{MODE_KEY}={mode}\n{WIDTH_KEY}={width}\n{HEIGHT_KEY}={height}");
}
