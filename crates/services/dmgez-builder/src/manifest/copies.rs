use std::path::Path;

#[cfg(any(unix, windows))]
use dmgez_config::CopySource;
use dmgez_config::VirtualPath;

#[cfg(any(unix, windows))]
use super::{Budget, FinderOverrides, walk::Walker};
use super::{Catalog, Error, Limits, Manifest};

#[cfg(any(unix, windows))]
const RESOLVE_OUTPUT: &str = "resolve output path";

/// One host source to copy into an existing image.
#[derive(Clone, Copy, Debug)]
pub struct CopyInput<'a> {
	/// Literal local path, relative to the edit batch directory when needed.
	pub source: &'a str,
	/// An explicit image path or the source basename at the image root.
	pub destination: Option<&'a VirtualPath>,
}

/// Discovers host additions using the same bounded, no-follow walk as a build.
/// The batch directory must be absolute. A relative output is resolved from
/// the current working directory. Synthetic image parents have no source.
/// # Errors
/// Rejects unsupported hosts, invalid or conflicting paths, output aliases,
/// inaccessible or changed sources, and inputs exceeding the discovery limits.
pub fn discover_copies(
	copies: &[CopyInput<'_>],
	config_dir: &Path,
	output: &Path,
	limits: Limits,
) -> Result<Manifest, Error> {
	if !config_dir.is_absolute() {
		return Err(Error::RelativeConfigDirectory);
	}
	let catalog = Catalog::new(limits);
	if copies.is_empty() {
		return catalog.finish();
	}
	#[cfg(any(unix, windows))]
	{
		let mut catalog = catalog;
		let output = std::path::absolute(output).map_err(|source| Error::Io {
			operation: RESOLVE_OUTPUT,
			path: output.to_owned(),
			source,
		})?;
		let mut walker = Walker::new(config_dir, Some(output));
		for (index, input) in copies.iter().enumerate() {
			let bytes = input
				.source
				.len()
				.checked_add(input.destination.map_or(0, |path| path.as_str().len()))
				.and_then(|bytes| u64::try_from(bytes).ok())
				.ok_or(Error::Limit(Budget::Metadata))?;
			if bytes > limits.max_metadata_bytes {
				return Err(Error::Limit(Budget::Metadata));
			}
			let (copy, destination) = CopySource::recursive_to(input.source, input.destination)
				.map_err(|source| Error::InvalidCopyInput { index, source })?;
			catalog.validate_destination(&destination)?;
			walker.copy(
				&destination,
				&copy,
				FinderOverrides::default(),
				&mut catalog,
				&mut || false,
			)?;
		}
		catalog.finish()
	}
	#[cfg(not(any(unix, windows)))]
	{
		let _ = output;
		Err(Error::UnsupportedHost)
	}
}
