use std::path::{Path, PathBuf};

use clap::Args;

use crate::CliResult;

const IMAGE_ARG: &str = "image";
const URL_ARG: &str = "url";
const RUN_LIMIT_ARG: &str = "run_limit";
pub(super) const REMOTE_FEATURE: &str = "remote";

#[derive(Args)]
pub(super) struct Arguments {
	/// Local disk image to inspect.
	#[arg(id = IMAGE_ARG, required_unless_present = URL_ARG)]
	image: Option<PathBuf>,
	/// Explicit HTTP or HTTPS image URL with byte ranges and a strong ETag.
	#[arg(long, id = URL_ARG, conflicts_with = IMAGE_ARG, value_name = "URL")]
	url: Option<String>,
	#[arg(long)]
	pub(super) json: bool,
	/// Report logical disk partitions or recognized partitionless geometry.
	#[arg(long)]
	pub(super) partitions: bool,
	/// First logical run in the optional detail page.
	#[arg(long, requires = RUN_LIMIT_ARG)]
	pub(super) run_offset: Option<usize>,
	/// Maximum number of runs in the optional detail page.
	#[arg(long, id = RUN_LIMIT_ARG)]
	pub(super) run_limit: Option<usize>,
}

pub(super) fn run(arguments: &Arguments) -> CliResult {
	match (arguments.image.as_deref(), arguments.url.as_deref()) {
		(Some(image), None) => local(arguments, image),
		(None, Some(url)) => remote(arguments, url),
		_ => Err("select one local image or one URL".into()),
	}
}

#[cfg(feature = "inspect")]
fn local(arguments: &Arguments, image: &Path) -> CliResult {
	crate::inspect::show(arguments, || Ok(crate::inspect::open_source(image)?))
}

#[cfg(not(feature = "inspect"))]
fn local(_: &Arguments, _: &Path) -> CliResult {
	Err("feature not compiled: inspect".into())
}

#[cfg(feature = "remote")]
fn remote(arguments: &Arguments, url: &str) -> CliResult {
	use dmgez_inspector::remote::{Limits, RemoteSource, http::HttpTransport};

	crate::inspect::show(arguments, || {
		Ok(RemoteSource::open(
			HttpTransport::new(url)?,
			Limits::default(),
		)?)
	})
}

#[cfg(not(feature = "remote"))]
fn remote(_: &Arguments, _: &str) -> CliResult {
	Err(format!("feature not compiled: {REMOTE_FEATURE}").into())
}
