use std::path::Path;

use dmgez_runtime::{disk::Encoding, preview::actual::image_png};

use crate::CliResult;

use super::{Arguments, print_report};

const ACTUAL_MODE: &str = "actual";
const STORE_KEY: &str = "finder_store";
const LAYOUT_KEY: &str = "finder_layout";
const BACKGROUND_KEY: &str = "background";
const OBSERVED_KEY: &str = "observed_positions";
const FALLBACK_KEY: &str = "fallback_positions";

pub(super) fn image(path: &Path, arguments: &Arguments) -> CliResult {
	let encoding = if arguments.raw {
		Encoding::Raw
	} else {
		Encoding::Udif(Default::default())
	};
	let report = image_png(
		path,
		encoding,
		arguments.partition,
		&arguments.folder.parse()?,
		arguments.visibility(),
		&arguments.output,
	)?;
	print_report(ACTUAL_MODE, report.size);
	let coverage = report.coverage;
	println!("{STORE_KEY}={}", coverage.store.as_str());
	println!("{LAYOUT_KEY}={}", coverage.layout.as_str());
	println!("{BACKGROUND_KEY}={}", coverage.background.as_str());
	println!("{OBSERVED_KEY}={}", coverage.observed_positions);
	println!("{FALLBACK_KEY}={}", coverage.fallback_positions);
	Ok(())
}
