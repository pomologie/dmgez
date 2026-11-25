use std::path::Path;

use dmgez_config::{LocalPath, Project};

use super::{Error, Limits, Manifest, SourceSnapshot, discover_for_output};
#[cfg(any(unix, windows))]
use super::{SourceKind, host};

/// Resolves the source tree for inspection without probing the image output.
/// Uses the same source identity checks and discovery limits as a build.
///
/// # Errors
/// Returns discovery errors and stops early when `cancelled` requests it.
pub fn discover_for_inspection(
	project: &Project,
	config_dir: &Path,
	limits: Limits,
	cancelled: impl FnMut() -> bool,
) -> Result<Manifest, Error> {
	discover_for_output(project, config_dir, limits, None, cancelled)
}

/// Captures regular assets against one project directory and shared source anchor.
pub struct AssetInspector<'config> {
	config_dir: &'config Path,
	#[cfg(any(unix, windows))]
	anchor: host::Anchor,
}

impl<'config> AssetInspector<'config> {
	/// Creates an asset inspector for an absolute project directory.
	///
	/// # Errors
	/// Fails when the directory is relative or the host source anchor cannot be opened.
	pub fn new(config_dir: &'config Path) -> Result<Self, Error> {
		if !config_dir.is_absolute() {
			return Err(Error::RelativeConfigDirectory);
		}
		#[cfg(any(unix, windows))]
		{
			Ok(Self {
				config_dir,
				anchor: host::Anchor::new()?,
			})
		}
		#[cfg(not(any(unix, windows)))]
		{
			Err(Error::UnsupportedHost)
		}
	}

	/// Captures a regular asset without following symlinks or reading its contents.
	///
	/// # Errors
	/// Returns source admission errors for missing, inaccessible, or non-regular
	/// assets and for paths the host cannot inspect safely.
	pub fn capture_regular(&self, input: &LocalPath) -> Result<SourceSnapshot, Error> {
		#[cfg(any(unix, windows))]
		{
			capture_regular(&self.anchor, input, self.config_dir)
		}
		#[cfg(not(any(unix, windows)))]
		{
			let _ = (input, self.config_dir);
			Err(Error::UnsupportedHost)
		}
	}
}

#[cfg(any(unix, windows))]
pub(super) fn capture_regular(
	anchor: &host::Anchor,
	input: &LocalPath,
	config_dir: &Path,
) -> Result<SourceSnapshot, Error> {
	let path = input.resolve(config_dir);
	let leaf = anchor.locate(&path)?;
	let snapshot = anchor.snapshot(&leaf.directory, leaf.name, path.clone())?;
	if snapshot.kind != SourceKind::RegularFile {
		return Err(Error::RegularSourceRequired { path });
	}
	Ok(snapshot)
}
