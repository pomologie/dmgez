use std::path::Path;

use dmgez_runtime::wrap;
use dmgez_udif::{
	Error,
	writer::{Profile, WriteOptions},
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
	let options = WriteOptions {
		profile,
		execution,
		..WriteOptions::default()
	};
	let report = wrap::file(input, destination, &options)?;
	println!(
		"wrote {} bytes from {} logical bytes",
		report.physical_bytes, report.logical_bytes
	);
	Ok(())
}
