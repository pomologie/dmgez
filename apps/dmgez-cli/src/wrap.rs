use std::{fs::File, path::Path};

use dmgez_core::{
	output::publish_new,
	source::{CapturedFile, SourceError},
};
use dmgez_udif::{
	Error,
	writer::{self, Execution, Profile, WriteOptions},
};

use crate::{CliResult, Format, PARALLEL_FEATURE};

const SERIAL_WORKER_COUNT: usize = 1;

pub(crate) fn run(
	input: &Path,
	destination: &Path,
	format: Format,
	serial: bool,
	threads: Option<usize>,
) -> CliResult {
	let input = CapturedFile::new(File::open(input)?)?;
	let execution = match (serial, cfg!(feature = "parallel"), threads) {
		(true, _, _)
		| (false, false, None | Some(Execution::AUTO_WORKERS | SERIAL_WORKER_COUNT)) => Execution::Serial,
		(false, false, Some(_)) => return Err(Error::FeatureNotCompiled(PARALLEL_FEATURE).into()),
		(false, true, workers) => Execution::Parallel {
			workers: workers.unwrap_or(Execution::AUTO_WORKERS),
		},
	};
	let profile = match format {
		Format::Udro => Profile::Udro,
		Format::Udzo => Profile::Udzo,
	};
	let options = WriteOptions {
		profile,
		execution,
		..WriteOptions::default()
	};
	let report = publish_new(
		destination,
		|stage| writer::write_image(input.reader(), input.len(), stage, &options),
		|_, _| {
			input.validate().map_err(|error| match error {
				SourceError::Changed => Error::SourceChanged,
				SourceError::Io(error) => error.into(),
				error => std::io::Error::other(error).into(),
			})
		},
	)?;
	println!(
		"wrote {} bytes from {} logical bytes",
		report.physical_bytes, report.logical_bytes
	);
	Ok(())
}
