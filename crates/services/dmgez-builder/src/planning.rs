//! Resolve build intent, sources, and generated assets before filesystem sealing.

use std::{
	fs, io,
	path::{Path, PathBuf},
};

use dmgez_config::Project;
#[cfg(feature = "finder-write")]
use dmgez_config::VirtualPath;
use dmgez_udif::writer::Cancellation;
use thiserror::Error;

use crate::{
	manifest::{self, Limits, Manifest, discover_for_output},
	metadata::sidecar::{self, Binding},
	preflight::{ExecutionProfile, OutputIntent, PreflightError, PreflightOptions, preflight},
	progress::BuildProgress,
};

#[cfg(feature = "finder-write")]
use crate::presentation::{self, DeferredFinderStore, GeneratedAsset, PreparedAssets};

#[cfg(feature = "volume-icon")]
use presentation::volume_icon::FINDER_INFO_BYTES;

/// Source manifest and generated assets ready for the selected filesystem engine.
/// Capacity remains subject to the filesystem's layout and allocation checks.
#[derive(Debug)]
pub struct PreparedBuild {
	profile: ExecutionProfile,
	manifest: Manifest,
	output_path: Option<PathBuf>,
	sidecar: Option<Binding>,
	#[cfg(feature = "finder-write")]
	finder_assets: PreparedAssets,
}

impl PreparedBuild {
	#[must_use]
	pub const fn profile(&self) -> &ExecutionProfile {
		&self.profile
	}

	#[must_use]
	pub const fn manifest(&self) -> &Manifest {
		&self.manifest
	}

	/// Path resolved against the admitted configuration directory, if used.
	#[must_use]
	pub fn output_path(&self) -> Option<&Path> {
		self.output_path.as_deref()
	}

	/// Captured AppleDouble source for a matching regular data source, if declared.
	#[must_use]
	pub fn metadata_sidecar_for(
		&self,
		source: &manifest::SourceSnapshot,
	) -> Option<&manifest::SourceSnapshot> {
		self.sidecar.as_ref()?.sidecar_for(source, &self.manifest)
	}

	#[cfg(feature = "finder-write")]
	#[must_use]
	pub fn finder_assets(&self) -> &[GeneratedAsset] {
		&self.finder_assets.ready
	}

	/// Generated Finder directories to reserve with the catalog manifest.
	#[cfg(feature = "finder-write")]
	#[must_use]
	pub fn finder_directories(&self) -> &[VirtualPath] {
		&self.finder_assets.directories
	}

	/// Finder stores whose contents require the frozen catalog identities.
	#[cfg(feature = "finder-write")]
	#[must_use]
	pub fn deferred_finder_stores(&self) -> &[DeferredFinderStore] {
		&self.finder_assets.deferred
	}

	/// Accounted metadata for pending layouts, excluding final store bytes and directories.
	#[cfg(feature = "finder-write")]
	#[must_use]
	pub const fn deferred_finder_metadata_bytes(&self) -> usize {
		self.finder_assets.deferred_metadata_bytes
	}

	/// Count of ready and deferred Finder assets, including item icons.
	#[cfg(feature = "finder-write")]
	#[must_use]
	pub fn finder_asset_count(&self) -> usize {
		self.finder_assets.count()
	}

	/// The encoded resource fork for a resolved item with a custom icon.
	#[cfg(feature = "volume-icon")]
	#[must_use]
	pub fn item_icon(&self, destination: &VirtualPath) -> Option<&[u8]> {
		self.finder_assets.item_icons.resource_fork(destination)
	}

	/// The hidden icon-file path required by a folder's custom icon.
	#[cfg(feature = "volume-icon")]
	#[must_use]
	pub fn item_icon_child(&self, destination: &VirtualPath) -> Option<&VirtualPath> {
		self.finder_assets.item_icons.folder_child(destination)
	}

	/// Accounted metadata retained for item icons, excluding the source manifest.
	#[cfg(feature = "volume-icon")]
	#[must_use]
	pub const fn item_icon_metadata_bytes(&self) -> usize {
		self.finder_assets.item_icons.metadata_bytes()
	}

	/// Merges the admitted volume icon's flag into the root catalog folder metadata.
	#[cfg(feature = "volume-icon")]
	pub fn apply_root_finder_info(&self, finder_info: &mut [u8; FINDER_INFO_BYTES]) {
		self.finder_assets.root_finder_info.apply(finder_info);
	}
}

/// An admission, discovery, or generated asset failure.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
	#[error(transparent)]
	Preflight(#[from] PreflightError),
	#[error("build preparation cancelled")]
	Cancelled,
	#[error("configuration directory must be an absolute path")]
	RelativeBase,
	#[error("could not admit the selected configuration directory")]
	Base(#[source] io::Error),
	#[error("the selected configuration directory is not a directory")]
	NotDirectory,
	#[error(transparent)]
	Manifest(#[from] manifest::Error),
	#[error(transparent)]
	MetadataSidecar(#[from] sidecar::Error),
	#[cfg(feature = "finder-write")]
	#[error(transparent)]
	Presentation(#[from] presentation::Error),
}

/// Preflights a build before accessing source paths, then freezes the source
/// tree and generated Finder assets. The caller explicitly selects the base
/// directory. Discovery resolves entries below the resulting directory without
/// following source links.
///
/// # Errors
/// Returns an error for unsupported build intent, cancellation, invalid source
/// identities, an invalid base, unrepresentable layout, or resource limits.
pub fn prepare(
	project: &Project,
	config_dir: &Path,
	options: PreflightOptions,
	cancellation: Cancellation,
) -> Result<PreparedBuild, Error> {
	prepare_with_progress(project, config_dir, options, cancellation, |_| {})
}

/// Prepares a build and reports source discovery and Finder asset progress.
///
/// # Errors
/// Returns the same failures as [`prepare`].
pub fn prepare_with_progress(
	project: &Project,
	config_dir: &Path,
	options: PreflightOptions,
	cancellation: Cancellation,
	progress: impl Fn(BuildProgress),
) -> Result<PreparedBuild, Error> {
	let mut profile = preflight(project, options)?;
	if cancellation.is_cancelled() {
		return Err(Error::Cancelled);
	}
	if !config_dir.is_absolute() {
		return Err(Error::RelativeBase);
	}
	let config_dir = fs::canonicalize(config_dir).map_err(Error::Base)?;
	if !fs::metadata(&config_dir).map_err(Error::Base)?.is_dir() {
		return Err(Error::NotDirectory);
	}
	profile.set_cancellation(cancellation);
	let limits = profile.limits();
	let max_entries = limits.max_entries as u64;
	let defaults = Limits::default();
	let path = (options.output == OutputIntent::Path).then_some(&project.image.output);
	progress(BuildProgress::Discovering);
	if profile.write_options().cancellation.is_cancelled() {
		return Err(Error::Cancelled);
	}
	let mut manifest = discover_for_output(
		project,
		&config_dir,
		Limits {
			max_entries,
			max_visited: defaults.max_visited.max(max_entries),
			max_metadata_bytes: limits.max_metadata_bytes as u64,
			..defaults
		},
		path,
		|| profile.write_options().cancellation.is_cancelled(),
	)?;
	progress(BuildProgress::SourcesDiscovered {
		entries: manifest.entries().len(),
		regular_payload_bytes: manifest.regular_payload_bytes(),
	});
	if profile.write_options().cancellation.is_cancelled() {
		return Err(Error::Cancelled);
	}
	let sidecar = if let Some(input) = &project.metadata.sidecar {
		Some(Binding::resolve(
			&mut manifest,
			input,
			&config_dir,
			path,
			limits.max_metadata_bytes as u64,
		)?)
	} else {
		None
	};
	#[cfg(feature = "finder-write")]
	let finder_assets = {
		#[cfg(feature = "volume-icon")]
		let volume_icon =
			presentation::volume_icon::capture_sources(&mut manifest, &profile, &config_dir, path)?;
		presentation::prepare(
			project,
			&mut manifest,
			&profile,
			&config_dir,
			path,
			#[cfg(feature = "volume-icon")]
			volume_icon,
		)?
	};
	#[cfg(feature = "finder-write")]
	if finder_assets.count() != 0 {
		progress(BuildProgress::FinderAssetsPrepared {
			assets: finder_assets.count(),
		});
		if profile.write_options().cancellation.is_cancelled() {
			return Err(Error::Cancelled);
		}
	}
	Ok(PreparedBuild {
		profile,
		manifest,
		output_path: path.map(|path| path.resolve(&config_dir)),
		sidecar,
		#[cfg(feature = "finder-write")]
		finder_assets,
	})
}

#[cfg(test)]
mod tests {
	use std::num::NonZeroUsize;

	use super::*;
	use crate::preflight::{Availability, BuildCapabilities, ReleaseIntent};

	const BASE_PROJECT: &str = "schema_version=1\n[image]\nname='Example'\nformat='udro'\n";

	fn options(output: OutputIntent) -> PreflightOptions {
		let capabilities = BuildCapabilities {
			hfsplus_unicode: Availability::Available,
			..BuildCapabilities::default()
		};
		PreflightOptions {
			capabilities,
			output,
			release: ReleaseIntent::Unsigned,
			host_threads: NonZeroUsize::MIN,
		}
	}

	#[test]
	fn preflights_and_checks_cancellation_before_accessing_the_base() {
		let project = Project::parse(&format!("{BASE_PROJECT}output='output.dmg'\n")).unwrap();
		let mut unsupported = options(OutputIntent::Path);
		unsupported.capabilities = BuildCapabilities::default();
		let base = Path::new("relative directory without access");
		let failed = prepare(&project, base, unsupported, Cancellation::default());
		assert!(matches!(
			failed,
			Err(Error::Preflight(PreflightError::FeatureNotCompiled(_)))
		));
		let cancellation = Cancellation::default();
		cancellation.cancel();
		let cancelled = prepare(&project, base, options(OutputIntent::Path), cancellation);
		assert!(matches!(cancelled, Err(Error::Cancelled)));
	}

	#[cfg(unix)]
	#[test]
	fn anchors_the_selected_directory_and_only_checks_path_aliases_for_path_output() {
		use std::os::unix::fs::symlink;

		const FILE_NAME: &str = "input";
		let scratch = tempfile::tempdir().unwrap();
		let directory = scratch.path().join("bundle");
		fs::create_dir(&directory).unwrap();
		fs::write(directory.join(FILE_NAME), b"file").unwrap();
		let alias = scratch.path().join("selected base");
		symlink(directory, &alias).unwrap();
		let project = Project::parse(&format!(
			"{BASE_PROJECT}output='{FILE_NAME}'\n[[contents]]\nsource='{FILE_NAME}'\n"
		))
		.unwrap();
		let path = prepare(
			&project,
			&alias,
			options(OutputIntent::Path),
			Cancellation::default(),
		);
		assert!(matches!(
			path,
			Err(Error::Manifest(manifest::Error::OutputSourceAlias { .. }))
		));
		let stream = prepare(
			&project,
			&alias,
			options(OutputIntent::Stream),
			Cancellation::default(),
		)
		.unwrap();
		assert!(stream.output_path().is_none());
		assert_eq!(stream.manifest().entries().len(), 1);
	}
}
