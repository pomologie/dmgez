use std::{
	fs::File,
	io::{self, ErrorKind, Read},
	path::Path,
};

use dmgez_config::{ConfigError, MAX_MANIFEST_BYTES};

use crate::CliResult;

const REGULAR_INPUT_REQUIRED: &str = "configuration input must be a regular file";

pub(crate) fn read(path: &Path) -> CliResult<String> {
	read_limited(path, MAX_MANIFEST_BYTES).map_err(|error| {
		if error.kind() == ErrorKind::FileTooLarge {
			ConfigError::TooLarge.into()
		} else {
			error.into()
		}
	})
}

pub(crate) fn read_limited(path: &Path, max_bytes: usize) -> io::Result<String> {
	let too_large = || {
		io::Error::new(
			ErrorKind::FileTooLarge,
			format!("configuration input exceeds the {max_bytes}-byte input limit"),
		)
	};
	if !path.metadata()?.is_file() {
		return Err(io::Error::new(
			ErrorKind::InvalidInput,
			REGULAR_INPUT_REQUIRED,
		));
	}
	let file = File::open(path)?;
	let metadata = file.metadata()?;
	if !metadata.is_file() {
		return Err(io::Error::new(
			ErrorKind::InvalidInput,
			REGULAR_INPUT_REQUIRED,
		));
	}
	let limit = max_bytes as u64;
	if metadata.len() > limit {
		return Err(too_large());
	}
	let mut source = String::new();
	file.take(limit + 1).read_to_string(&mut source)?;
	if source.len() > max_bytes {
		return Err(too_large());
	}
	Ok(source)
}

#[cfg(feature = "project-build")]
pub(crate) fn definitions(raw: &[String]) -> CliResult<std::collections::BTreeMap<String, String>> {
	const DEFINITION_SEPARATOR: char = '=';
	const INVALID_DEFINITION: &str = "expected a definition in NAME=VALUE form";
	let mut defines = std::collections::BTreeMap::new();
	for definition in raw {
		let (name, value) = definition
			.split_once(DEFINITION_SEPARATOR)
			.ok_or(INVALID_DEFINITION)?;
		defines.insert(name.to_owned(), value.to_owned());
	}
	Ok(defines)
}
