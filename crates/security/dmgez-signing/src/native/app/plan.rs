use std::{
	collections::BTreeSet,
	fs::{self, Metadata},
	io,
	os::unix::fs::MetadataExt,
	path::{Path, PathBuf},
};

use thiserror::Error;

use super::macho::{self, Kind};

mod bundle;

#[cfg(test)]
mod tests;

const MAX_ENTRIES: usize = 100_000;
const MAX_DEPTH: usize = 64;
const MAX_ITEMS: usize = 4_096;
const MAX_MANIFEST_BYTES: u64 = 32 << 20;
const FRAMEWORK: &str = "framework";
const VERSIONS: &str = "Versions";
const CURRENT_VERSION: &str = "Current";

#[derive(Debug, Eq, PartialEq)]
pub(super) struct Item {
	/// Name relative to the application. The application itself has an empty path.
	pub(super) relative: PathBuf,
	/// Actual file named by the native signature's executable field.
	pub(super) executable: PathBuf,
	pub(super) hardened: bool,
}

#[derive(Debug, Error)]
pub(super) enum Error {
	#[error("could not inspect bundle entry {path:?}")]
	Io {
		path: PathBuf,
		#[source]
		source: io::Error,
	},
	#[error("could not classify native code at {path:?}")]
	Mach {
		path: PathBuf,
		#[source]
		source: Box<dyn std::error::Error + Send + Sync>,
	},
	#[error("malformed or unsupported code bundle at {0:?}")]
	Bundle(PathBuf),
	#[error("the bundle executable at {0:?} is hard-linked")]
	LinkedExecutable(PathBuf),
	#[error("unsupported filesystem entry in the bundle at {0:?}")]
	SpecialEntry(PathBuf),
	#[error("bundle signing plan exceeded the {0} limit")]
	Limit(&'static str),
}

pub(super) fn build(root: &Path) -> Result<Vec<Item>, Error> {
	if !metadata(root)?.is_dir() {
		return Err(Error::Bundle(root.to_path_buf()));
	}
	let mut walk = Walk {
		root,
		remaining: MAX_ENTRIES,
		manifest_bytes: MAX_MANIFEST_BYTES,
		bundles: Vec::new(),
		files: Vec::new(),
	};
	walk.directory(Path::new(""), 0, Directory::Bundle)?;
	let covered: BTreeSet<_> = walk
		.bundles
		.iter()
		.map(|item| metadata(&root.join(&item.executable)).map(|entry| identity(&entry)))
		.collect::<Result<_, _>>()?;
	walk.files.retain(|(entry, _)| !covered.contains(entry));
	if walk.bundles.len() + walk.files.len() > MAX_ITEMS {
		return Err(Error::Limit("code item count"));
	}
	walk.bundles
		.extend(walk.files.into_iter().map(|(_, item)| item));
	walk.bundles.sort_unstable_by(|left, right| {
		right
			.relative
			.components()
			.count()
			.cmp(&left.relative.components().count())
			.then_with(|| left.relative.cmp(&right.relative))
	});
	Ok(walk.bundles)
}

#[derive(Clone, Copy)]
enum Directory {
	Ordinary,
	Bundle,
	Framework,
	VersionedFramework,
	Versions,
	FrameworkVersion,
}

struct Walk<'a> {
	root: &'a Path,
	remaining: usize,
	manifest_bytes: u64,
	bundles: Vec<Item>,
	files: Vec<((u64, u64), Item)>,
}

impl Walk<'_> {
	fn directory(
		&mut self,
		relative: &Path,
		depth: usize,
		directory: Directory,
	) -> Result<(), Error> {
		if depth > MAX_DEPTH {
			return Err(Error::Limit("directory depth"));
		}
		let path = self.root.join(relative);
		let directory = match directory {
			Directory::Framework if optional_metadata(&path.join(VERSIONS))?.is_some() => {
				Directory::VersionedFramework
			}
			other => other,
		};
		let item = match directory {
			Directory::Bundle => bundle::ordinary(self.root, relative, &mut self.manifest_bytes)?,
			Directory::Framework | Directory::FrameworkVersion => Some(bundle::framework(
				self.root,
				relative,
				&mut self.manifest_bytes,
			)?),
			_ => None,
		};
		if let Some(item) = item {
			if self.bundles.len() == MAX_ITEMS {
				return Err(Error::Limit("bundle count"));
			}
			self.bundles.push(item);
		}

		let mut children = Vec::new();
		for entry in fs::read_dir(&path).map_err(|source| io_error(&path, source))? {
			self.remaining = self
				.remaining
				.checked_sub(1)
				.ok_or(Error::Limit("filesystem entry count"))?;
			let entry = entry.map_err(|source| io_error(&path, source))?;
			children.push(entry.file_name());
		}
		children.sort_unstable();
		if matches!(directory, Directory::Versions) {
			check_versions(&path, &children)?;
		}
		for child in children {
			let relative = relative.join(child);
			let path = self.root.join(&relative);
			let metadata = metadata(&path)?;
			let kind = metadata.file_type();
			if kind.is_symlink() {
				if matches!(directory, Directory::VersionedFramework)
					&& relative.ends_with(VERSIONS)
					|| matches!(directory, Directory::Versions)
						&& !relative.ends_with(CURRENT_VERSION)
				{
					return Err(Error::Bundle(path));
				}
				continue;
			}
			let child_directory = match directory {
				Directory::VersionedFramework if relative.ends_with(VERSIONS) && kind.is_dir() => {
					Directory::Versions
				}
				Directory::Versions if !relative.ends_with(CURRENT_VERSION) && kind.is_dir() => {
					Directory::FrameworkVersion
				}
				Directory::VersionedFramework | Directory::Versions => {
					return Err(Error::Bundle(path));
				}
				_ => directory_for(&relative),
			};
			if kind.is_dir() {
				self.directory(&relative, depth + 1, child_directory)?;
			} else if kind.is_file() {
				self.file(relative, &path, &metadata)?;
			} else {
				return Err(Error::SpecialEntry(path));
			}
		}
		Ok(())
	}

	fn file(&mut self, relative: PathBuf, path: &Path, metadata: &Metadata) -> Result<(), Error> {
		let kind = classify(path)?;
		let script = kind.is_none()
			&& bundle::is_code_location(&relative)
			&& metadata.mode() & bundle::EXECUTE_BITS != 0
			&& bundle::is_script(path)?;
		if kind.is_some() || script {
			require_single_link(path, metadata)?;
			if self.files.len() == 2 * MAX_ITEMS {
				return Err(Error::Limit("native code file count"));
			}
			self.files.push((
				identity(metadata),
				Item {
					executable: relative.clone(),
					relative,
					hardened: matches!(kind, Some(Kind::Executable)),
				},
			));
		}
		Ok(())
	}
}

fn check_versions(path: &Path, children: &[std::ffi::OsString]) -> Result<(), Error> {
	let current = path.join(CURRENT_VERSION);
	let target = fs::read_link(&current).map_err(|source| io_error(&current, source))?;
	let valid = target
		.file_name()
		.filter(|name| *name == target.as_os_str() && *name != CURRENT_VERSION)
		.is_some_and(|name| children.iter().any(|child| child == name));
	if !valid {
		return Err(Error::Bundle(current));
	}
	Ok(())
}

fn directory_for(path: &Path) -> Directory {
	let Some(extension) = path.extension() else {
		return Directory::Ordinary;
	};
	if extension.eq_ignore_ascii_case(FRAMEWORK) {
		Directory::Framework
	} else if bundle::is_bundle_extension(extension) {
		Directory::Bundle
	} else {
		Directory::Ordinary
	}
}

fn metadata(path: &Path) -> Result<Metadata, Error> {
	fs::symlink_metadata(path).map_err(|source| io_error(path, source))
}

fn optional_metadata(path: &Path) -> Result<Option<Metadata>, Error> {
	match fs::symlink_metadata(path) {
		Ok(value) => Ok(Some(value)),
		Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
		Err(source) => Err(io_error(path, source)),
	}
}

fn io_error(path: &Path, source: io::Error) -> Error {
	Error::Io {
		path: path.to_path_buf(),
		source,
	}
}

fn classify(path: &Path) -> Result<Option<Kind>, Error> {
	macho::classify(path).map_err(|source| Error::Mach {
		path: path.to_path_buf(),
		source: Box::new(source),
	})
}

fn require_single_link(path: &Path, metadata: &Metadata) -> Result<(), Error> {
	if metadata.nlink() == 1 {
		Ok(())
	} else {
		Err(Error::LinkedExecutable(path.to_path_buf()))
	}
}

fn identity(metadata: &Metadata) -> (u64, u64) {
	(metadata.dev(), metadata.ino())
}
