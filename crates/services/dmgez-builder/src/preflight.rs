//! Pure build capability negotiation before the manifest walker opens sources.

use std::{num::NonZeroUsize, ops::RangeInclusive};

use dmgez_config::{
	AutoFlag, Capacity, CompressionCache, Filesystem, ImageFormat, Partition, Project,
};
use dmgez_udif::writer::{Execution, Profile, WriteOptions};
use thiserror::Error;

mod capabilities;
mod finder;
mod profile;
#[cfg(test)]
mod tests;

pub use capabilities::{Availability, BuildCapabilities, BuildFeature};
use finder::{admit_volume_icon, finder_plan};
pub use profile::{ExecutionProfile, ResourceLimits};

const PRINTABLE_ASCII: RangeInclusive<u8> = b' '..=b'~';
const DEFAULT_MEMORY_BYTES: usize = 512 << 20;
const DEFAULT_METADATA_BYTES: usize = 128 << 20;
const DEFAULT_MAX_ENTRIES: usize = 1_000_000;
const DEFAULT_METADATA_BUDGET_DIVISOR: usize = 4;

/// Intent for the output boundary of the operation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum OutputIntent {
	/// Stage and publish to a destination file.
	#[default]
	Path,
	/// Write to a caller-owned stream without atomic publication.
	Stream,
}

/// Explicit user authorization for release stages.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ReleaseIntent {
	/// Plan an unsigned image build.
	#[default]
	Unsigned,
	/// Plan an artifact for the explicitly requested release workflow.
	Release,
}

/// Inputs already known to the composer. Admission does no host probing itself.
#[derive(Clone, Copy, Debug)]
pub struct PreflightOptions {
	pub capabilities: BuildCapabilities,
	pub output: OutputIntent,
	pub release: ReleaseIntent,
	pub host_threads: NonZeroUsize,
}

/// The filesystem writer profile admitted for this operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum FilesystemProfile {
	HfsplusAscii,
	HfsplusUnicode,
}

/// Publication semantics for the admitted operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutputPlan {
	Path { overwrite: bool },
	Stream,
}

/// Release behavior a caller can include in its plan or report.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReleasePlan {
	Unsigned,
	/// Enabled release stages in the project require explicit opt-in.
	ConfiguredStagesDeferred,
	/// Hand the staged artifact to the application-owned release workflow.
	Requested,
}

/// The default view of the image root's planned Finder store.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FinderPlan {
	/// The build will not generate Finder stores.
	NoStore,
	/// Open the root in icon view.
	IconView,
	/// Open the root in list view.
	ListView,
	/// Open the root in column view.
	ColumnView,
}

/// A resource constrained by configuration or host representation.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[non_exhaustive]
pub enum Resource {
	#[error("worker count")]
	Workers,
	#[error("aggregate working memory")]
	Memory,
	#[error("metadata bytes")]
	Metadata,
	#[error("manifest entries")]
	Entries,
	#[error("UDIF runs")]
	Runs,
}

/// Finder settings that the initial writer profile cannot faithfully emit.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[non_exhaustive]
pub enum FinderSetting {
	#[error("volume icon representation")]
	VolumeIcon,
	#[error("custom item icon")]
	CustomIcon,
	/// Legacy diagnostic retained for callers of the initial image background profile.
	#[error("separate Retina background asset")]
	BackgroundRetina,
}

/// A reason configuration cannot enter the source and build stages.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum PreflightError {
	#[error("feature not compiled: {0}")]
	FeatureNotCompiled(BuildFeature),
	#[error("backend unavailable: {0}")]
	BackendUnavailable(BuildFeature),
	#[error("unsupported filesystem: {0:?}")]
	Filesystem(Filesystem),
	#[error("unsupported image format: {0:?}")]
	Format(ImageFormat),
	#[error("unsupported partition layout: {0:?}")]
	Partition(Partition),
	#[error("volume label requires an unsupported name profile")]
	VolumeLabel,
	#[error("content entry {index} requires an unsupported name profile")]
	Destination { index: usize },
	#[error("compression settings require the UDZO profile")]
	Compression,
	#[error("compression cache requires the UDZO profile")]
	CacheFormat,
	#[error("compression cache requires a prepared build with a resolved cache directory")]
	CacheRequiresPreparedBuild,
	#[error("more than one thread conflicts with explicitly disabled parallelism")]
	SerialWorkers,
	#[error("a stream has no destination file to overwrite")]
	OverwriteStream,
	#[error(
		"metadata sidecar requires preserving Finder information, resource forks, or attributes"
	)]
	MetadataSidecarIgnored,
	#[error("pre-publication verification requires a staged path output")]
	VerifyStream,
	#[error("release stages require a staged path output")]
	ReleaseStream,
	#[error("resource limit: {0}")]
	ResourceLimit(Resource),
	#[error("planned logical size violates the configured capacity")]
	Capacity,
	#[error("unsupported Finder setting: {0}")]
	UnsupportedFinder(FinderSetting),
	#[error("duplicate Finder position for content entry {index}")]
	DuplicatePosition { index: usize },
	#[error("duplicate case-equivalent Finder position names in a folder")]
	DuplicatePositionName,
	#[error("Finder folder {index} requires an unsupported name profile")]
	FinderFolderName { index: usize },
	#[error("Finder folder {index} duplicates an earlier folder under HFS+ comparison")]
	DuplicateFinderFolder { index: usize },
	#[cfg(feature = "finder-write")]
	#[error("unsupported Finder layout")]
	Finder(#[from] dmgez_finder::Error),
	#[error(transparent)]
	Writer(#[from] dmgez_udif::Error),
}

/// Resolves and validates the initial HFS+ and UDIF build profile without I/O.
///
/// # Errors
///
/// Returns typed configuration, backend, or budget failures before any
/// source access, output staging, or release operation.
pub fn preflight(
	project: &Project,
	options: PreflightOptions,
) -> Result<ExecutionProfile, PreflightError> {
	let capabilities = options.capabilities;
	if project.image.filesystem != Filesystem::Hfsplus {
		return Err(PreflightError::Filesystem(project.image.filesystem));
	}
	let filesystem = filesystem_profile(capabilities)?;
	if project.image.partition != Partition::None {
		return Err(PreflightError::Partition(project.image.partition));
	}
	let profile = match project.image.format {
		ImageFormat::Udro => Profile::Udro,
		ImageFormat::Udzo => Profile::Udzo,
		ImageFormat::Ulfo => Profile::Ulfo,
		ImageFormat::Ulmo => Profile::Ulmo,
		format => return Err(PreflightError::Format(format)),
	};
	if profile == Profile::Udzo && !cfg!(feature = "zlib") {
		return Err(PreflightError::FeatureNotCompiled(BuildFeature::Zlib));
	}
	if profile == Profile::Ulfo && !cfg!(feature = "lzfse") {
		return Err(PreflightError::FeatureNotCompiled(BuildFeature::Lzfse));
	}
	if profile == Profile::Ulmo && !cfg!(feature = "lzma") {
		return Err(PreflightError::FeatureNotCompiled(BuildFeature::Lzma));
	}
	if profile != Profile::Udzo
		&& (project.compression.level.is_some() || project.compression.block_size.is_some())
	{
		return Err(PreflightError::Compression);
	}
	if filesystem == FilesystemProfile::HfsplusAscii {
		check_known_names(project)?;
	}
	let output = output_plan(project, options)?;
	let release = release_plan(project, options)?;
	let cache = cache_plan(project, profile, release)?;
	let finder = finder_plan(project, capabilities)?;
	let volume_icon = admit_volume_icon(project, capabilities)?;
	if project.license.is_some() {
		return Err(PreflightError::FeatureNotCompiled(BuildFeature::License));
	}
	if project.metadata.sidecar.is_some() {
		capabilities
			.metadata_sidecar
			.require(BuildFeature::MetadataSidecar)?;
		if !cfg!(any(target_os = "linux", target_os = "macos")) {
			return Err(PreflightError::BackendUnavailable(
				BuildFeature::MetadataSidecar,
			));
		}
		let requests = &project.metadata;
		if !(requests.preserve_finder_info
			|| requests.preserve_resource_forks
			|| requests.preserve_extended_attributes)
		{
			return Err(PreflightError::MetadataSidecarIgnored);
		}
	}

	let limits = limits(project)?;
	let writer = writer_options(project, profile, limits, options.host_threads, cache)?;
	let result = ExecutionProfile {
		filesystem,
		capacity: project.image.capacity(),
		free_space_bytes: project.image.free_space.get(),
		output,
		verify_output: project.build.verify_output,
		release,
		cache: cache.cloned(),
		finder,
		volume_icon,
		limits,
		reproducible: project.build.reproducible,
		source_date_epoch: project.build.source_date_epoch,
		fail_on_metadata_loss: project.build.fail_on_metadata_loss(&project.metadata),
		writer,
	};
	if let Capacity::Exact(bytes) = result.capacity {
		result.validate_logical_bytes(bytes.get())?;
	}
	Ok(result)
}

fn filesystem_profile(
	capabilities: BuildCapabilities,
) -> Result<FilesystemProfile, PreflightError> {
	match (capabilities.hfsplus_unicode, capabilities.hfsplus_ascii) {
		(Availability::Available, _) => Ok(FilesystemProfile::HfsplusUnicode),
		(_, Availability::Available) => Ok(FilesystemProfile::HfsplusAscii),
		(Availability::Unavailable, _) | (_, Availability::Unavailable) => {
			Err(PreflightError::BackendUnavailable(BuildFeature::Hfsplus))
		}
		_ => Err(PreflightError::FeatureNotCompiled(BuildFeature::Hfsplus)),
	}
}

fn check_known_names(project: &Project) -> Result<(), PreflightError> {
	if !printable_ascii(project.image.name.as_str()) {
		return Err(PreflightError::VolumeLabel);
	}
	for (index, entry) in project.contents.iter().enumerate() {
		if !entry.destination.components().all(printable_ascii) {
			return Err(PreflightError::Destination { index });
		}
	}
	Ok(())
}

fn printable_ascii(name: &str) -> bool {
	name.bytes().all(|byte| PRINTABLE_ASCII.contains(&byte))
}

fn output_plan(project: &Project, options: PreflightOptions) -> Result<OutputPlan, PreflightError> {
	let verify = project.build.verify_output;
	let overwrite = project.build.overwrite;
	if options.output == OutputIntent::Stream {
		if overwrite {
			return Err(PreflightError::OverwriteStream);
		}
		if verify {
			return Err(PreflightError::VerifyStream);
		}
		return Ok(OutputPlan::Stream);
	}
	if verify {
		options.capabilities.verify.require(BuildFeature::Verify)?;
	}
	if overwrite {
		options
			.capabilities
			.atomic_replace
			.require(BuildFeature::AtomicReplace)?;
	}
	Ok(OutputPlan::Path { overwrite })
}

fn release_plan(
	project: &Project,
	options: PreflightOptions,
) -> Result<ReleasePlan, PreflightError> {
	if options.release == ReleaseIntent::Release {
		if options.output == OutputIntent::Stream {
			return Err(PreflightError::ReleaseStream);
		}
		options
			.capabilities
			.release
			.require(BuildFeature::Release)?;
		return Ok(ReleasePlan::Requested);
	}
	let deferred = project
		.signing
		.as_ref()
		.is_some_and(|stage| stage.enabled == Some(true))
		|| project
			.notarization
			.as_ref()
			.is_some_and(|stage| stage.enabled == Some(true))
		|| project.release.as_ref().is_some_and(|stage| {
			[
				stage.verify,
				stage.require_notarization,
				stage.require_native_assessment,
			]
			.contains(&Some(true))
		});
	Ok(if deferred {
		ReleasePlan::ConfiguredStagesDeferred
	} else {
		ReleasePlan::Unsigned
	})
}

fn cache_plan(
	project: &Project,
	profile: Profile,
	release: ReleasePlan,
) -> Result<Option<&CompressionCache>, PreflightError> {
	let Some(cache) = &project.performance.cache else {
		return Ok(None);
	};
	// Release stages must encode the original filesystem bytes independently.
	if release == ReleasePlan::Requested {
		return Ok(None);
	}
	if profile != Profile::Udzo {
		return Err(PreflightError::CacheFormat);
	}
	if !cfg!(feature = "compression-cache") {
		return Err(PreflightError::FeatureNotCompiled(
			BuildFeature::CompressionCache,
		));
	}
	if !cfg!(any(target_os = "linux", target_os = "macos")) {
		return Err(PreflightError::BackendUnavailable(
			BuildFeature::CompressionCache,
		));
	}
	Ok(Some(cache))
}

fn limits(project: &Project) -> Result<ResourceLimits, PreflightError> {
	let performance = &project.performance;
	let bound = |value: u64, resource| {
		usize::try_from(value)
			.ok()
			.filter(|&bound| bound != 0)
			.ok_or(PreflightError::ResourceLimit(resource))
	};
	let memory_bytes = performance
		.memory_limit
		.map_or(Ok(DEFAULT_MEMORY_BYTES), |bytes| {
			bound(bytes.get(), Resource::Memory)
		})?;
	let adaptive_metadata_default =
		DEFAULT_METADATA_BYTES.min(memory_bytes / DEFAULT_METADATA_BUDGET_DIVISOR);
	let max_metadata_bytes = performance
		.max_metadata_bytes
		.map_or(Ok(adaptive_metadata_default), |bytes| {
			bound(bytes.get(), Resource::Metadata)
		})?;
	if max_metadata_bytes >= memory_bytes || max_metadata_bytes == 0 {
		return Err(PreflightError::ResourceLimit(Resource::Memory));
	}
	let max_entries = performance
		.max_entries
		.map_or(Ok(DEFAULT_MAX_ENTRIES), |value| {
			bound(value.get(), Resource::Entries)
		})?;
	if project.contents.len() > max_entries {
		return Err(PreflightError::ResourceLimit(Resource::Entries));
	}
	let max_runs = performance
		.max_runs
		.map_or(WriteOptions::MAX_DATA_RUNS, |runs| {
			runs.get().min(WriteOptions::MAX_DATA_RUNS as u64) as usize
		});
	Ok(ResourceLimits {
		memory_bytes,
		max_metadata_bytes,
		max_entries,
		max_runs,
	})
}

fn writer_options(
	project: &Project,
	profile: Profile,
	limits: ResourceLimits,
	host_threads: NonZeroUsize,
	cache: Option<&CompressionCache>,
) -> Result<WriteOptions, PreflightError> {
	let threads = usize::try_from(project.performance.threads)
		.map_err(|_| PreflightError::ResourceLimit(Resource::Workers))?;
	let parallel = project.performance.parallel;
	let explicit_parallel = parallel == AutoFlag::Enabled || threads > NonZeroUsize::MIN.get();
	if parallel == AutoFlag::Disabled && threads > NonZeroUsize::MIN.get() {
		return Err(PreflightError::SerialWorkers);
	}
	if explicit_parallel && !cfg!(feature = "parallel") {
		return Err(PreflightError::FeatureNotCompiled(BuildFeature::Parallel));
	}
	let execution = if cfg!(feature = "parallel") && parallel != AutoFlag::Disabled {
		Execution::Parallel { workers: threads }
	} else {
		Execution::Serial
	};
	let mut writer = WriteOptions {
		profile,
		execution,
		max_buffer_bytes: limits.memory_bytes - limits.max_metadata_bytes,
		..WriteOptions::default()
	};
	if let Some(chunk) = project.compression.block_size {
		writer.chunk_bytes = usize::try_from(chunk.get())
			.map_err(|_| PreflightError::ResourceLimit(Resource::Memory))?;
	}
	if let Some(level) = project.compression.level {
		writer.zlib_level = level;
	}
	let admission = match cache {
		None => writer.admit(host_threads)?,
		#[cfg(feature = "compression-cache")]
		Some(_) => writer.admit_with_cache(host_threads)?,
		#[cfg(not(feature = "compression-cache"))]
		Some(_) => {
			return Err(PreflightError::FeatureNotCompiled(
				BuildFeature::CompressionCache,
			));
		}
	};
	writer.execution = admission.execution();
	Ok(writer)
}
