use std::path::PathBuf;

use clap::Args;

use crate::CliResult;

pub(super) const FEATURE: &str = "preview";
const ROOT_FOLDER: &str = "/";

#[derive(Args)]
pub(super) struct Arguments {
	/// TOML project whose declared design will be rendered.
	#[arg(long)]
	project: PathBuf,
	/// New path for the PNG. Existing files are preserved.
	#[arg(short, long)]
	output: PathBuf,
	/// Virtual folder to render, defaults to the image root.
	#[arg(long, default_value = ROOT_FOLDER)]
	folder: String,
	/// Show entries marked hidden in the project.
	#[arg(long)]
	show_hidden: bool,
}

#[cfg(feature = "preview")]
pub(super) fn run(arguments: &Arguments) -> CliResult {
	use std::path::Path;

	use dmgez_config::Project;
	use dmgez_core::output::publish_new;
	use dmgez_preview::{ItemVisibility, raster::DesignPreview};

	const MODE_KEY: &str = "mode";
	const WIDTH_KEY: &str = "width";
	const HEIGHT_KEY: &str = "height";
	const DESIGN_MODE: &str = "design";

	let folder = arguments.folder.parse()?;
	let project_directory = arguments.project.parent().unwrap_or(Path::new(""));
	let project = Project::parse(&crate::project_input::read(&arguments.project)?)?;
	let visibility = if arguments.show_hidden {
		ItemVisibility::IncludeHidden
	} else {
		ItemVisibility::Visible
	};
	let preview = DesignPreview::from_project(&project, project_directory, &folder, visibility)?;
	publish_new(
		&arguments.output,
		|output| preview.write_png(output),
		|_, ()| Ok(()),
	)?;
	let (width, height) = preview.size();
	println!("{MODE_KEY}={DESIGN_MODE}\n{WIDTH_KEY}={width}\n{HEIGHT_KEY}={height}");
	Ok(())
}

#[cfg(not(feature = "preview"))]
pub(super) fn run(_: &Arguments) -> CliResult {
	Err(format!("feature not compiled: {FEATURE}").into())
}
