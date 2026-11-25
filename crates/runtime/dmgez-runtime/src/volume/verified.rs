use std::{fs::File, io, path::Path};

use dmgez_udif::{
	Error,
	reader::{FileSource, ReadAt},
};

/// Opens a named source for use under the held-file qualification policy.
///
/// # Errors
/// Returns source open, qualification, or callback errors.
pub fn with_verified_path<T, E: From<io::Error> + From<Error>>(
	image: &Path,
	read: impl FnOnce(&FileSource, &dyn Fn() -> io::Result<()>) -> Result<T, E>,
) -> Result<T, E> {
	with_verified_file(&File::open(image)?, read)
}

/// Qualifies a held source and supplies the same check for prepublication.
///
/// The check requires supported quarantine evidence and a stable file stamp.
/// Workflows that publish data must call it at their publication boundary.
///
/// # Errors
/// Returns source qualification or callback errors.
pub fn with_verified_file<T, E: From<io::Error> + From<Error>>(
	file: &File,
	read: impl FnOnce(&FileSource, &dyn Fn() -> io::Result<()>) -> Result<T, E>,
) -> Result<T, E> {
	let source = FileSource::new(file.try_clone()?)?;
	let verify_source = || {
		dmgez_operations::extract::host::check_source_quarantine(file)?;
		source.check_unchanged().map_err(io::Error::other)
	};
	verify_source()?;
	read(&source, &verify_source)
}
