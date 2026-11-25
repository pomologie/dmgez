use std::{
	io::{self, Write},
	path::Path,
};

use dmgez_operations::{
	convert,
	optimize::{Evaluation, Options, SampleLimits, file},
};
use dmgez_udif::writer::WriteOptions;

use crate::{CliResult, EncodingFlags, SamplingFlags, convert::write_report};

const SAMPLE_CHUNKS_KEY: &str = "sampled_chunks";
const SAMPLE_BYTES_KEY: &str = "sampled_decoded_bytes";
const SAMPLE_KEY: &str = "sample";
const CANDIDATE_KEY: &str = "candidate";

pub(crate) fn run(
	source: &Path,
	destination: &Path,
	sampling: SamplingFlags,
	encoding: &EncodingFlags,
) -> CliResult {
	let defaults = SampleLimits::default();
	let options = Options {
		conversion: convert::Options {
			output: WriteOptions {
				execution: encoding.execution()?,
				..WriteOptions::default()
			},
			..convert::Options::default()
		},
		sample_limits: SampleLimits {
			max_chunks: sampling.max_sample_chunks.unwrap_or(defaults.max_chunks),
			max_decoded_bytes: sampling
				.max_sample_bytes
				.unwrap_or(defaults.max_decoded_bytes),
		},
	};
	let report = file(source, destination, &options)?;
	let mut stdout = io::stdout().lock();
	writeln!(stdout, "{SAMPLE_CHUNKS_KEY} = {}", report.windows.len())?;
	writeln!(
		stdout,
		"{SAMPLE_BYTES_KEY} = {}",
		report.sampled_decoded_bytes
	)?;
	for window in &report.windows {
		writeln!(
			stdout,
			"{SAMPLE_KEY} = offset {} bytes {}",
			window.offset, window.bytes
		)?;
	}
	for candidate in &report.candidates {
		let profile = candidate.profile;
		match &candidate.evaluation {
			Evaluation::Measured {
				wrapped_bytes,
				elapsed,
			} => writeln!(
				stdout,
				"{CANDIDATE_KEY} = {profile:?} wrapped_bytes {wrapped_bytes} encode_us {}",
				elapsed.as_micros(),
			)?,
			Evaluation::Unavailable(error) => writeln!(
				stdout,
				"{CANDIDATE_KEY} = {profile:?} unavailable ({error})"
			)?,
			_ => writeln!(stdout, "{CANDIDATE_KEY} = {profile:?}")?,
		}
	}
	write_report(&mut stdout, &report.conversion)?;
	Ok(())
}
