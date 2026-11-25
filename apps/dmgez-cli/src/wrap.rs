use std::{fs::File, path::Path};

use dmgez_core::{
	output::publish_new,
	source::{CapturedFile, SourceError},
};
use dmgez_udif::{
	Error,
	writer::{self, Profile, WriteOptions},
};

use crate::{CliResult, EncodingFlags, Format, LZFSE_FEATURE, LZMA_FEATURE};

pub(crate) fn run(
	input: &Path,
	destination: &Path,
	format: Format,
	encoding: &EncodingFlags,
) -> CliResult {
	let profile = match format {
		Format::Udro => Profile::Udro,
		Format::Udzo => Profile::Udzo,
		Format::Ulfo if cfg!(feature = "lzfse") => Profile::Ulfo,
		Format::Ulfo => return Err(Error::FeatureNotCompiled(LZFSE_FEATURE).into()),
		Format::Ulmo if cfg!(feature = "lzma") => Profile::Ulmo,
		Format::Ulmo => return Err(Error::FeatureNotCompiled(LZMA_FEATURE).into()),
	};
	let execution = encoding.execution()?;
	let input = CapturedFile::new(File::open(input)?)?;
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
