use std::{fs::File, path::Path};

use crate::SignError;

#[cfg(any(target_os = "macos", windows))]
const MAX_BYTES: usize = 4 << 10;

#[cfg(target_os = "macos")]
const ATTRIBUTE: &str = "com.apple.quarantine";

#[cfg(target_os = "macos")]
pub(super) fn read(file: &File, _: &Path) -> Result<Option<Vec<u8>>, SignError> {
	use rustix::{fs, io::Errno};

	let mut bytes = [0; MAX_BYTES];
	match fs::fgetxattr(file, ATTRIBUTE, &mut bytes[..]) {
		Ok(length) => Ok(Some(bytes[..length].to_vec())),
		Err(Errno::NOATTR) => Ok(None),
		Err(error) => Err(SignError::Quarantine(error.into())),
	}
}

#[cfg(target_os = "macos")]
pub(super) fn write(file: &File, _: &Path, value: Option<&[u8]>) -> Result<(), SignError> {
	use rustix::fs;

	if let Some(value) = value {
		fs::fsetxattr(file, ATTRIBUTE, value, fs::XattrFlags::empty())
			.map_err(|error| SignError::Quarantine(error.into()))?;
	}
	Ok(())
}

#[cfg(windows)]
const ZONE_STREAM_SUFFIX: &str = ":Zone.Identifier";

#[cfg(windows)]
fn stream_path(path: &Path) -> std::ffi::OsString {
	let mut stream = path.as_os_str().to_owned();
	stream.push(ZONE_STREAM_SUFFIX);
	stream
}

#[cfg(windows)]
pub(super) fn check_rename(directory: &Path) -> Result<(), SignError> {
	use std::{fs, io};

	const PROBE_NAME: &str = "zone-rename-probe";
	const PROBE_STREAM: &[u8] = b"[ZoneTransfer]\r\nZoneId=3\r\n";
	let staged = tempfile::NamedTempFile::new_in(directory).map_err(SignError::Quarantine)?;
	write(staged.as_file(), staged.path(), Some(PROBE_STREAM))?;
	let path = directory.join(PROBE_NAME);
	let renamed = staged
		.persist_noclobber(&path)
		.map_err(|error| SignError::Quarantine(error.error))?;
	if read(&renamed, &path)?.as_deref() != Some(PROBE_STREAM) {
		return Err(SignError::Quarantine(io::Error::new(
			io::ErrorKind::Unsupported,
			"output filesystem did not preserve Zone.Identifier through rename",
		)));
	}
	drop(renamed);
	fs::remove_file(path).map_err(SignError::Quarantine)
}

#[cfg(windows)]
pub(super) fn read(_: &File, path: &Path) -> Result<Option<Vec<u8>>, SignError> {
	use std::io::{self, Read};

	let file = match File::open(stream_path(path)) {
		Ok(file) => file,
		Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
		Err(error) => return Err(SignError::Quarantine(error)),
	};
	let mut value = Vec::new();
	file.take(MAX_BYTES as u64 + 1)
		.read_to_end(&mut value)
		.map_err(SignError::Quarantine)?;
	if value.len() > MAX_BYTES {
		return Err(SignError::Quarantine(io::Error::new(
			io::ErrorKind::InvalidData,
			"Zone.Identifier exceeds the signing metadata limit",
		)));
	}
	Ok(Some(value))
}

#[cfg(windows)]
pub(super) fn write(_: &File, path: &Path, value: Option<&[u8]>) -> Result<(), SignError> {
	use std::{fs::OpenOptions, io::Write};

	if let Some(value) = value {
		let mut stream = OpenOptions::new()
			.write(true)
			.create_new(true)
			.open(stream_path(path))
			.map_err(SignError::Quarantine)?;
		stream.write_all(value).map_err(SignError::Quarantine)?;
		stream.sync_all().map_err(SignError::Quarantine)?;
	}
	Ok(())
}

#[cfg(not(any(target_os = "macos", windows)))]
pub(super) fn read(_: &File, _: &Path) -> Result<Option<Vec<u8>>, SignError> {
	Ok(None)
}

#[cfg(not(any(target_os = "macos", windows)))]
pub(super) fn write(_: &File, _: &Path, _: Option<&[u8]>) -> Result<(), SignError> {
	Ok(())
}
