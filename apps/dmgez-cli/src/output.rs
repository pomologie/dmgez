use std::{error::Error, fs::File, path::Path};

pub(crate) fn publish<T>(
	destination: &Path,
	write: impl FnOnce(&mut File) -> Result<T, Box<dyn Error>>,
) -> Result<T, Box<dyn Error>> {
	let parent = destination
		.parent()
		.filter(|path| !path.as_os_str().is_empty())
		.unwrap_or_else(|| Path::new("."));
	let mut stage = tempfile::NamedTempFile::new_in(parent)?;
	let report = write(stage.as_file_mut())?;
	stage.as_file().sync_all()?;
	stage
		.persist_noclobber(destination)
		.map_err(|failure| failure.error)?;
	Ok(report)
}
