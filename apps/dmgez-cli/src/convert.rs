use std::path::Path;

use dmgez_operations::convert::{Options, file};
use dmgez_udif::writer::{Profile, WriteOptions};

use crate::{CliResult, ConversionFormat, EncodingFlags, report_field};

const INPUT_PROFILE_KEY: &str = "input_profile";
const OUTPUT_PROFILE_KEY: &str = "output_profile";
const LOGICAL_SIZE_KEY: &str = "logical_bytes";
const PHYSICAL_SIZE_KEY: &str = "physical_bytes";
const OUTPUT_CHECK_KEY: &str = "output_check";
const TRUST_KEY: &str = "trust";

pub(crate) fn run(
	source: &Path,
	destination: &Path,
	format: ConversionFormat,
	encoding: &EncodingFlags,
) -> CliResult {
	let profile = match format {
		ConversionFormat::Udro => Profile::Udro,
		ConversionFormat::Udzo => Profile::Udzo,
	};
	let options = Options {
		output: WriteOptions {
			profile,
			execution: encoding.execution()?,
			..WriteOptions::default()
		},
		..Options::default()
	};
	let report = file(source, destination, &options)?;
	report_field(
		INPUT_PROFILE_KEY,
		format_args!("{:?}", report.source_profile),
	);
	report_field(
		OUTPUT_PROFILE_KEY,
		format_args!("{:?}", report.output_profile),
	);
	report_field(LOGICAL_SIZE_KEY, report.output.logical_bytes);
	report_field(PHYSICAL_SIZE_KEY, report.output.physical_bytes);
	report_field(OUTPUT_CHECK_KEY, format_args!("{:?}", report.output_check));
	report_field(TRUST_KEY, format_args!("{:?}", report.trust));
	Ok(())
}
