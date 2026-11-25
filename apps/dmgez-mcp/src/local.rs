use std::{fs::File, io, path::Path};

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(any(target_os = "linux", target_os = "macos"))]
use unix as host;
#[cfg(windows)]
use windows as host;

pub(super) use host::LocalRoot;
use host::Stamp;

pub(super) const MAX_FILE_NAME_BYTES: usize = 255;
const FORBIDDEN_FILE_NAME: [char; 3] = ['/', '\\', ':'];

#[derive(Debug)]
pub(super) enum FileAdmission {
	Unavailable,
	Changed,
}

pub(super) struct FileBinding<'a> {
	root: &'a LocalRoot,
	name: &'a Path,
	stamp: Stamp,
}

#[derive(Debug)]
pub(super) enum ScanError {
	Unavailable,
	Changed,
	Limit,
}

impl From<io::Error> for ScanError {
	fn from(_: io::Error) -> Self {
		Self::Unavailable
	}
}

impl LocalRoot {
	pub(super) fn open_file<'a>(
		&'a self,
		name: &'a Path,
	) -> Result<(File, FileBinding<'a>), FileAdmission> {
		let named = self
			.named_stamp(name)
			.map_err(|_| FileAdmission::Unavailable)?;
		if !named.is_direct_file() {
			return Err(FileAdmission::Unavailable);
		}
		let file = self
			.open_data(name)
			.map_err(|_| FileAdmission::Unavailable)?;
		let stamp = Stamp::capture(&file).map_err(|_| FileAdmission::Unavailable)?;
		if named != stamp {
			return Err(FileAdmission::Changed);
		}
		Ok((
			file,
			FileBinding {
				root: self,
				name,
				stamp,
			},
		))
	}

	pub(super) fn matching_names(
		&self,
		limit: usize,
		accepts: impl Fn(&str) -> bool,
	) -> Result<Vec<String>, ScanError> {
		let before = self.directory_stamp()?;
		let mut directory = self.directory()?;
		let mut names = Vec::new();
		for index in 0..=limit {
			let Some(entry) = directory.next_entry()? else {
				break;
			};
			if index == limit {
				return Err(ScanError::Limit);
			}
			if let Some(name) = entry.name().to_str()
				&& accepts(name)
				&& self.is_direct_entry(&entry)?
			{
				names.push(name.to_owned());
			}
		}
		if self.directory_stamp()? != before {
			return Err(ScanError::Changed);
		}
		Ok(names)
	}
}

impl FileBinding<'_> {
	pub(super) fn verify(&self) -> Result<(), FileAdmission> {
		let observed = self.root.named_stamp(self.name).map_err(|error| {
			if error.kind() == io::ErrorKind::NotFound {
				FileAdmission::Changed
			} else {
				FileAdmission::Unavailable
			}
		})?;
		if self.stamp == observed {
			Ok(())
		} else {
			Err(FileAdmission::Changed)
		}
	}
}

pub(super) fn direct_name<'a>(file_name: &'a str, extension: &str) -> Option<&'a Path> {
	let path = Path::new(file_name);
	if file_name.len() > MAX_FILE_NAME_BYTES
		|| file_name.contains(FORBIDDEN_FILE_NAME)
		|| file_name.chars().any(char::is_control)
		|| path.extension() != Some(extension.as_ref())
	{
		return None;
	}
	#[cfg(windows)]
	host::validate_component(path.as_os_str()).ok()?;
	Some(path)
}

#[cfg(test)]
mod tests;
