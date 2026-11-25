#[cfg(any(unix, windows))]
use std::{
	borrow::Cow,
	io::ErrorKind,
	path::{Component, Path, PathBuf},
};

#[cfg(any(unix, windows))]
use super::{Budget, SourceKind, host};
use super::{Error, SourceSnapshot};

#[cfg(all(test, unix))]
mod tests;

#[cfg(any(unix, windows))]
const BUNDLE_CONTENTS: &str = "Contents";
#[cfg(any(unix, windows))]
const FRAMEWORK_VERSIONS: &str = "Versions";
#[cfg(any(unix, windows))]
const MAX_FRAMEWORK_VERSION_ENTRIES: usize = 128;
#[cfg(any(unix, windows))]
const CODE_SIGNATURE_DIRECTORY: &str = "_CodeSignature";
#[cfg(any(unix, windows))]
const CODE_RESOURCES: &str = "CodeResources";

impl SourceSnapshot {
	/// Checks conventional code-signing markers around a selected source.
	/// Anchor traversal opens parent directories without following links.
	pub(crate) fn has_signed_bundle_marker(&self) -> Result<bool, Error> {
		#[cfg(any(unix, windows))]
		{
			let anchor = host::Anchor::new()?;
			let path = normalized_path(self.path());
			if self.kind == SourceKind::Directory
				&& (marker_exists(&anchor, &path)?
					|| marker_exists(&anchor, &path.join(BUNDLE_CONTENTS))?)
			{
				return Ok(true);
			}
			for parent in path.ancestors().skip(1) {
				if marker_exists(&anchor, parent)? {
					return Ok(true);
				}
			}
			if self.kind != SourceKind::Directory {
				return Ok(false);
			}
			if framework_versions_have_marker(&anchor, &path.join(FRAMEWORK_VERSIONS))? {
				return Ok(true);
			}
			if path
				.file_name()
				.is_some_and(|name| name.eq_ignore_ascii_case(FRAMEWORK_VERSIONS))
			{
				return framework_versions_have_marker(&anchor, &path);
			}
			Ok(false)
		}
		#[cfg(not(any(unix, windows)))]
		{
			Err(Error::UnsupportedHost)
		}
	}
}

#[cfg(any(unix, windows))]
fn normalized_path(path: &Path) -> Cow<'_, Path> {
	if !path.components().any(|part| part == Component::ParentDir) {
		return Cow::Borrowed(path);
	}
	// Source discovery has already checked every traversed directory without
	// following symlinks. Ignore lexical ancestors left behind by '..'.
	let mut normalized = PathBuf::new();
	for part in path.components() {
		if part == Component::ParentDir {
			normalized.pop();
		} else {
			normalized.push(part.as_os_str());
		}
	}
	Cow::Owned(normalized)
}

#[cfg(any(unix, windows))]
fn marker_exists(anchor: &host::Anchor, directory: &Path) -> Result<bool, Error> {
	let marker = directory
		.join(CODE_SIGNATURE_DIRECTORY)
		.join(CODE_RESOURCES);
	let Some(leaf) = optional_lookup(anchor.locate(&marker))? else {
		return Ok(false);
	};
	Ok(
		optional_lookup(anchor.snapshot(&leaf.directory, leaf.name, marker.clone()))?
			.is_some_and(|source| source.kind == SourceKind::RegularFile),
	)
}

#[cfg(any(unix, windows))]
fn framework_versions_have_marker(anchor: &host::Anchor, path: &Path) -> Result<bool, Error> {
	let Some(leaf) = optional_lookup(anchor.locate(path))? else {
		return Ok(false);
	};
	let Some(source) = optional_lookup(anchor.snapshot(&leaf.directory, leaf.name, path.into()))?
		.filter(|source| source.kind == SourceKind::Directory)
	else {
		return Ok(false);
	};
	let mut directory = host::open_directory(&leaf.directory, leaf.name, &source)?;
	let mut remaining = MAX_FRAMEWORK_VERSION_ENTRIES;
	while let Some(name) = host::next_name(&mut directory, &source)? {
		remaining = remaining
			.checked_sub(1)
			.ok_or(Error::Limit(Budget::Visited))?;
		// The version must be a physical directory. The marker lookup
		// traverses it as a parent component, so it skips aliases such as Current.
		if marker_exists(anchor, &path.join(name))? {
			return Ok(true);
		}
	}
	host::match_opened(&source, host::directory_handle(&directory, &source)?)?;
	source.validate()?;
	Ok(false)
}

#[cfg(any(unix, windows))]
fn optional_lookup<T>(result: Result<T, Error>) -> Result<Option<T>, Error> {
	match result {
		// Windows snapshots refuse reparse points and devices as sources.
		Err(Error::UnsupportedSource { .. }) => Ok(None),
		Err(Error::Io { source, .. })
			if matches!(
				source.kind(),
				ErrorKind::NotFound | ErrorKind::NotADirectory
			) =>
		{
			Ok(None)
		}
		#[cfg(unix)]
		Err(Error::Io { source, .. })
			if source.raw_os_error() == Some(rustix::io::Errno::LOOP.raw_os_error()) =>
		{
			Ok(None)
		}
		other => other.map(Some),
	}
}
