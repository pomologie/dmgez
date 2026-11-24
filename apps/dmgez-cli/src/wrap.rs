use std::{
	fs::{File, Metadata},
	path::Path,
};

use dmgez_udif::{
	Error,
	writer::{self, Execution, Profile, WriteOptions},
};

use crate::{CliResult, Format, PARALLEL_FEATURE, output};

const SERIAL_WORKER_COUNT: usize = 1;

pub(crate) fn run(
	input: &Path,
	destination: &Path,
	format: Format,
	serial: bool,
	threads: Option<usize>,
) -> CliResult {
	let mut input = File::open(input)?;
	let before = input.metadata()?;
	if !before.is_file() {
		return Err("raw source must be a regular file".into());
	}
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
	let report = output::publish(destination, |stage| {
		let report = writer::write_image(&mut input, before.len(), stage, &options)?;
		if changed(&before, &input.metadata()?)? {
			return Err(Error::SourceChanged.into());
		}
		Ok(report)
	})?;
	println!(
		"wrote {} bytes from {} logical bytes",
		report.physical_bytes, report.logical_bytes
	);
	Ok(())
}

fn changed(before: &Metadata, after: &Metadata) -> std::io::Result<bool> {
	#[cfg(unix)]
	{
		use std::os::unix::fs::MetadataExt;
		if (before.ctime(), before.ctime_nsec()) != (after.ctime(), after.ctime_nsec()) {
			return Ok(true);
		}
	}
	Ok(before.len() != after.len() || before.modified()? != after.modified()?)
}
