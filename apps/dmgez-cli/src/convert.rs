use std::{
	io::{self, Write},
	path::Path,
};

use dmgez_operations::convert::{Options, Report, SignaturePolicy, file_with_signature_policy};
use dmgez_udif::writer::{Profile, WriteOptions};

use crate::{CliResult, ConversionFormat, EncodingFlags};

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
	allow_signature_loss: bool,
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
	let policy = if allow_signature_loss {
		SignaturePolicy::Discard
	} else {
		SignaturePolicy::RequireUnsigned
	};
	let report = file_with_signature_policy(source, destination, &options, policy)?;
	write_report(io::stdout().lock(), &report)?;
	Ok(())
}

pub(crate) fn write_report(mut output: impl Write, report: &Report) -> io::Result<()> {
	writeln!(output, "{INPUT_PROFILE_KEY} = {:?}", report.source_profile)?;
	writeln!(output, "{OUTPUT_PROFILE_KEY} = {:?}", report.output_profile)?;
	writeln!(
		output,
		"{LOGICAL_SIZE_KEY} = {}",
		report.output.logical_bytes
	)?;
	writeln!(
		output,
		"{PHYSICAL_SIZE_KEY} = {}",
		report.output.physical_bytes
	)?;
	writeln!(output, "{OUTPUT_CHECK_KEY} = {:?}", report.output_check)?;
	writeln!(output, "{TRUST_KEY} = {:?}", report.trust)
}
