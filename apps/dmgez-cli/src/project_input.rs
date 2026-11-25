use std::{fs::File, io::Read, path::Path};

use dmgez_config::{ConfigError, MAX_MANIFEST_BYTES};

use crate::CliResult;

const REGULAR_INPUT_REQUIRED: &str = "configuration input must be a regular file";

pub(crate) fn read(path: &Path) -> CliResult<String> {
	if !path.metadata()?.is_file() {
		return Err(REGULAR_INPUT_REQUIRED.into());
	}
	let file = File::open(path)?;
	let metadata = file.metadata()?;
	if !metadata.is_file() {
		return Err(REGULAR_INPUT_REQUIRED.into());
	}
	let limit = MAX_MANIFEST_BYTES as u64;
	if metadata.len() > limit {
		return Err(ConfigError::TooLarge.into());
	}
	let mut source = String::new();
	file.take(limit + 1).read_to_string(&mut source)?;
	if source.len() > MAX_MANIFEST_BYTES {
		return Err(ConfigError::TooLarge.into());
	}
	Ok(source)
}
