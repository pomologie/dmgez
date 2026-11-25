use std::path::{Path, PathBuf};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::{collections::BTreeSet, fs, io, os::unix::fs::MetadataExt};

use dmgez_cache::DiskCache;
use thiserror::Error;

use super::{ExecutionProfile, Manifest};

/// A configured compression cache could not be safely associated with this build.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
	#[error("could not open the configured compression cache {path:?}")]
	Open {
		path: PathBuf,
		#[source]
		source: dmgez_cache::Error,
	},
	#[error("compression cache overlaps a build source or output: {0:?}")]
	Overlap(PathBuf),
	#[cfg(any(target_os = "linux", target_os = "macos"))]
	#[error("could not establish compression cache separation for {path:?}")]
	Inspect {
		path: PathBuf,
		#[source]
		source: io::Error,
	},
	#[error("the compression cache path checks exceed the build metadata budget")]
	Budget,
}

pub(super) fn prepare(
	profile: &ExecutionProfile,
	project_dir: &Path,
	manifest: &Manifest,
	output: Option<&Path>,
) -> Result<Option<DiskCache>, Error> {
	let Some(settings) = profile.cache() else {
		return Ok(None);
	};
	let directory = settings.directory.resolve(project_dir);
	let cache = DiskCache::open(&directory, settings.max_bytes).map_err(|source| Error::Open {
		path: directory,
		source,
	})?;
	check_sources(&cache, manifest, profile)?;
	if let Some(output) = output {
		check_output(&cache, output)?;
	}
	Ok(Some(cache))
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn check_sources(
	cache: &DiskCache,
	manifest: &Manifest,
	profile: &ExecutionProfile,
) -> Result<(), Error> {
	const PATH_INDEX_CHARGE: usize = 2 * size_of::<&Path>();
	let retained = usize::try_from(manifest.metadata_bytes()).map_err(|_| Error::Budget)?;
	let mut remaining = profile.limits().max_metadata_bytes.saturating_sub(retained);
	let mut checked = BTreeSet::new();
	for source in manifest.sources() {
		let identity = source.metadata().identity;
		if source.is_directory()
			&& cache
				.ancestors()
				.contains(&(identity.device, identity.inode))
		{
			return Err(Error::Overlap(source.path().to_owned()));
		}
		for parent in source.path().ancestors().skip(1) {
			if checked.contains(parent) {
				break;
			}
			check_directory(cache, parent, source.path())?;
			remaining = remaining
				.checked_sub(PATH_INDEX_CHARGE)
				.ok_or(Error::Budget)?;
			checked.insert(parent);
		}
	}
	Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn check_sources(
	_cache: &DiskCache,
	_manifest: &Manifest,
	_profile: &ExecutionProfile,
) -> Result<(), Error> {
	Ok(())
}

pub(crate) fn check_output(cache: &DiskCache, output: &Path) -> Result<(), Error> {
	#[cfg(any(target_os = "linux", target_os = "macos"))]
	{
		let absolute = std::path::absolute(output).map_err(|source| Error::Inspect {
			path: output.to_owned(),
			source,
		})?;
		for ancestor in absolute.ancestors() {
			// A not-yet-created destination is still scoped by its existing ancestors.
			match check_directory(cache, ancestor, output) {
				Err(Error::Inspect { source, .. }) if source.kind() == io::ErrorKind::NotFound => {}
				result => result?,
			}
		}
	}
	#[cfg(not(any(target_os = "linux", target_os = "macos")))]
	let _ = (cache, output);
	Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn check_directory(cache: &DiskCache, directory: &Path, path: &Path) -> Result<(), Error> {
	let metadata = fs::metadata(directory).map_err(|source| Error::Inspect {
		path: directory.to_owned(),
		source,
	})?;
	// Darwin device IDs are signed 32-bit values in stat. Compare their bits
	// in the same representation as the held cache directory descriptor.
	#[cfg(target_os = "macos")]
	let device = u64::from(metadata.dev() as u32);
	#[cfg(target_os = "linux")]
	let device = metadata.dev();
	if (device, metadata.ino()) == cache.directory_identity() {
		return Err(Error::Overlap(path.to_owned()));
	}
	Ok(())
}
