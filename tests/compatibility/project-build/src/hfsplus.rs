#![cfg(any(target_os = "macos", target_os = "linux"))]

use std::{collections::TryReserveError, io::Write, mem::size_of, path::Path};

#[cfg(feature = "finder-write")]
use std::borrow::Cow;

use dmgez_builder::{
	execution::{BuildPlan, BuildReport, ExecuteError, OutputVerifier},
	metadata::{self as host_metadata, Attribute, policy},
	planning::{self, PreparedBuild},
	preflight::{
		Availability, BuildCapabilities, ExecutionProfile, FilesystemProfile, PreflightError,
		PreflightOptions,
	},
};
use dmgez_config::{Capacity, Project, VirtualPath};
use dmgez_udif::writer::{Cancellation, WriteOptions};
#[cfg(feature = "finder-write")]
use hfsplus_builder::volume::{EntryKind, ForkSource};
use hfsplus_builder::{
	SealedPlan,
	volume::{self, CapacityError, CapacityPolicy, ManifestEntry, VolumeError, VolumeOptions},
};
use hfsplus_core::ByteLen;
use thiserror::Error;

mod metadata;
mod source;

#[cfg(feature = "finder-write")]
use metadata::DEFAULT_FILE_MODE;
use metadata::{Capture, DEFAULT_FOLDER_MODE, MISSING_HFS_DATE, epoch_seconds, generated_metadata};
use source::Payload;

const ALLOCATION_BLOCK_BYTES: u32 = 4096;
const IMAGE_PATH_SEPARATOR: char = '/';

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
	#[error(transparent)]
	Prepare(#[from] planning::Error),
	#[error(transparent)]
	Filesystem(#[from] VolumeError),
	#[error(transparent)]
	Capacity(#[from] CapacityError),
	#[error(transparent)]
	Profile(#[from] PreflightError),
	#[error(transparent)]
	Execute(#[from] ExecuteError),
	#[error("build was cancelled while sealing the HFS+ image")]
	Cancelled,
	#[error("build metadata exceeds the available byte budget")]
	MetadataBudget,
	#[error("could not reserve bounded adapter metadata")]
	Reserve(#[from] TryReserveError),
	#[error("HFS+ adapter received an unsupported build profile")]
	UnsupportedProfile,
	#[error("virtual path is outside the HFS+ manifest profile: {path:?}")]
	Path { path: String },
	#[error("source metadata policy failed for {path:?}")]
	Policy {
		path: String,
		#[source]
		cause: policy::Error,
	},
	#[error("hard-link grouping failed")]
	HardLinks(#[source] policy::Error),
	#[error("could not capture regular-source metadata for {path:?}")]
	Capture {
		path: String,
		#[source]
		cause: host_metadata::Error,
	},
	#[error("unsupported source kind or missing source at {path:?}")]
	UnsupportedEntry { path: String },
	#[error("requested Finder flag overrides are unavailable at {path:?}")]
	FinderFlags { path: String },
	#[error("HFS+ timestamp exceeds the representable epoch")]
	Timestamp,
	#[error("admitted build has no output path")]
	OutputPath,
	#[error("admitted attribute name cannot be represented in HFS+")]
	AttributeName(#[from] std::str::Utf8Error),
}

/// One explicit metadata omission or loss and the corresponding image entry.
#[derive(Debug)]
pub struct MetadataLoss<'build> {
	pub path: &'build VirtualPath,
	pub details: Vec<policy::Loss>,
}

/// The immutable association of the project policy with its prepared build.
pub struct PreparedHfsBuild<'project> {
	project: &'project Project,
	build: PreparedBuild,
}

/// An HFS+ image with frozen geometry and borrowed source identities.
pub struct HfsImage<'build> {
	prepared: &'build PreparedBuild,
	image: SealedPlan<Payload<'build>>,
	metadata_losses: Vec<MetadataLoss<'build>>,
}

/// A completed build and its per-entry metadata losses.
#[derive(Debug)]
pub struct HfsBuildReport<'build> {
	pub report: BuildReport,
	pub metadata_losses: Vec<MetadataLoss<'build>>,
}

/// Capabilities owned by this adapter. Composers may supply independent staged verification.
#[must_use]
pub fn capabilities() -> BuildCapabilities {
	let mut capabilities = BuildCapabilities::default();
	capabilities.hfsplus_unicode = Availability::Available;
	capabilities.hfsplus_ascii = Availability::Available;
	if cfg!(feature = "finder-write") {
		capabilities.finder_icon_view = Availability::Available;
	}
	capabilities
}

/// Admit the request while retaining the same immutable project policy for sealing.
///
/// # Errors
/// Returns preflight, discovery, presentation, or cancellation errors.
pub fn prepare<'project>(
	project: &'project Project,
	config_dir: &Path,
	options: PreflightOptions,
	cancellation: Cancellation,
) -> Result<PreparedHfsBuild<'project>, Error> {
	let build = planning::prepare(project, config_dir, options, cancellation)?;
	Ok(PreparedHfsBuild { project, build })
}

impl PreparedHfsBuild<'_> {
	/// Resolve exact HFS+ catalog, attributes, geometry, and stream placements.
	///
	/// # Errors
	/// Returns source-change, metadata, path, capacity, or cancellation errors.
	pub fn seal(&self) -> Result<HfsImage<'_>, Error> {
		seal(self.project, &self.build)
	}
}

impl<'build> HfsImage<'build> {
	/// Exact raw volume length, independent of final compression.
	#[must_use]
	pub fn logical_bytes(&self) -> u64 {
		self.image.image_len().0
	}

	#[must_use]
	pub fn metadata_losses(&self) -> &[MetadataLoss<'build>] {
		&self.metadata_losses
	}

	/// Publish to the admitted project output through same-directory staging.
	///
	/// # Errors
	/// Fails on invalid output intent, source mutation, verification, cancellation, or I/O.
	pub fn publish(
		self,
		verifier: Option<&OutputVerifier<'_>>,
	) -> Result<HfsBuildReport<'build>, Error> {
		let output = self.prepared.output_path().ok_or(Error::OutputPath)?;
		let build = BuildPlan::new(
			self.prepared.profile(),
			self.prepared.manifest(),
			self.image.reader(),
			self.logical_bytes(),
		)?;
		let report = build.publish(output, verifier)?;
		Ok(HfsBuildReport {
			report,
			metadata_losses: self.metadata_losses,
		})
	}

	/// Write the UDIF image to the admitted sink without a raw intermediate.
	///
	/// # Errors
	/// Fails on invalid output intent, source mutation, cancellation, or I/O.
	pub fn write_to(self, sink: impl Write) -> Result<HfsBuildReport<'build>, Error> {
		let build = BuildPlan::new(
			self.prepared.profile(),
			self.prepared.manifest(),
			self.image.reader(),
			self.logical_bytes(),
		)?;
		let report = build.write_to(sink)?;
		Ok(HfsBuildReport {
			report,
			metadata_losses: self.metadata_losses,
		})
	}
}

fn seal<'build>(
	project: &Project,
	prepared: &'build PreparedBuild,
) -> Result<HfsImage<'build>, Error> {
	let profile = prepared.profile();
	let manifest = prepared.manifest();
	let cancellation = &profile.write_options().cancellation;
	check_cancelled(cancellation)?;
	let policy = policy::Policy::new(&project.metadata, profile.fail_on_metadata_loss());
	let generated_seconds = profile
		.source_date_epoch()
		.map(epoch_seconds)
		.transpose()?
		.unwrap_or(MISSING_HFS_DATE);
	let max_metadata = profile.limits().max_metadata_bytes;
	let mut remaining = max_metadata;
	charge(
		&mut remaining,
		usize::try_from(manifest.metadata_bytes()).map_err(|_| Error::MetadataBudget)?,
	)?;
	let links = policy::HardLinks::discover(manifest, remaining).map_err(Error::HardLinks)?;
	charge(&mut remaining, links.accounted_bytes())?;

	#[cfg(feature = "finder-write")]
	let generated = prepared.finder_assets();
	#[cfg(feature = "finder-write")]
	for asset in generated {
		charge(&mut remaining, asset.bytes.capacity())?;
		charge(&mut remaining, asset.path.as_str().len())?;
		charge(
			&mut remaining,
			size_of::<dmgez_builder::presentation::GeneratedAsset>(),
		)?;
	}

	let count = manifest.entries().len();
	#[cfg(feature = "finder-write")]
	let count = count
		.checked_add(generated.len())
		.ok_or(Error::MetadataBudget)?;
	charge_many::<ManifestEntry<'_, Payload<'_>>>(&mut remaining, count)?;
	let mut entries = Vec::new();
	entries.try_reserve_exact(count)?;
	let copied_entries = manifest.entries().len();
	charge_many::<MetadataLoss<'_>>(&mut remaining, copied_entries)?;
	let mut losses = Vec::new();
	losses.try_reserve_exact(copied_entries)?;
	charge_many::<(&str, Vec<Attribute>)>(&mut remaining, copied_entries)?;
	let mut pending_attrs = Vec::new();
	pending_attrs.try_reserve_exact(copied_entries)?;
	let mut capture = Capture {
		policy: &policy,
		links: &links,
		project,
		remaining: &mut remaining,
		losses: &mut losses,
	};
	for entry in manifest.entries() {
		check_cancelled(cancellation)?;
		let (entry, attrs) = capture.entry(entry, generated_seconds)?;
		if !attrs.is_empty() {
			pending_attrs.push((entry.path, attrs));
		}
		entries.push(entry);
	}
	#[cfg(feature = "finder-write")]
	for asset in generated {
		check_cancelled(cancellation)?;
		entries.push(ManifestEntry {
			path: relative(&asset.path)?,
			metadata: generated_metadata(DEFAULT_FILE_MODE, generated_seconds),
			kind: EntryKind::GeneratedFile { resource: None },
		});
	}

	let mut options =
		VolumeOptions::for_capacity(project.image.name.as_str(), ALLOCATION_BLOCK_BYTES)?;
	options.max_entries = profile.limits().max_entries;
	options.limits.max_metadata_bytes =
		ByteLen(u64::try_from(remaining).map_err(|_| Error::MetadataBudget)?);
	options.root = generated_metadata(DEFAULT_FOLDER_MODE, generated_seconds);
	options.dates = [
		generated_seconds,
		generated_seconds,
		MISSING_HFS_DATE,
		MISSING_HFS_DATE,
	];

	let mut draft = match profile.filesystem() {
		FilesystemProfile::HfsplusUnicode => volume::plan(options, entries)?,
		FilesystemProfile::HfsplusAscii => volume::plan_ascii(options, entries)?,
		_ => return Err(Error::UnsupportedProfile),
	};
	#[cfg(feature = "finder-write")]
	for asset in generated {
		check_cancelled(cancellation)?;
		draft.bind_generated_data(
			relative(&asset.path)?,
			ForkSource::whole(Payload::Bytes(Cow::Borrowed(&asset.bytes))),
		)?;
	}
	for (path, attrs) in pending_attrs {
		for attr in attrs {
			check_cancelled(cancellation)?;
			draft.add_inline_attribute(path, std::str::from_utf8(&attr.name)?, &attr.value)?;
		}
	}
	let image = draft.seal_with_capacity(capacity(profile)?)?;
	profile.validate_logical_bytes(image.image_len().0)?;
	check_cancelled(cancellation)?;
	Ok(HfsImage {
		prepared,
		image,
		metadata_losses: losses,
	})
}

fn capacity(profile: &ExecutionProfile) -> Result<CapacityPolicy, Error> {
	let free_space = ByteLen(profile.free_space_bytes());
	let write = profile.write_options();
	let max_runs = profile.limits().max_runs.min(WriteOptions::MAX_DATA_RUNS);
	let run_bound = (max_runs as u64).saturating_mul(write.chunk_bytes as u64);
	let writer_max = run_bound.min(write.max_logical_bytes);

	Ok(match profile.capacity() {
		Capacity::Auto { .. } => CapacityPolicy::ShrinkWithin {
			max_bytes: ByteLen(writer_max),
			free_space,
		},
		Capacity::Exact(bytes) => CapacityPolicy::Exact {
			bytes: ByteLen(bytes.get()),
			free_space,
		},
		Capacity::ShrinkWithin(bytes) => CapacityPolicy::ShrinkWithin {
			max_bytes: ByteLen(bytes.get().min(writer_max)),
			free_space,
		},
		_ => return Err(Error::UnsupportedProfile),
	})
}

fn relative(path: &VirtualPath) -> Result<&str, Error> {
	path.as_str()
		.strip_prefix(IMAGE_PATH_SEPARATOR)
		.filter(|relative| !relative.is_empty())
		.ok_or_else(|| Error::Path {
			path: path.as_str().into(),
		})
}

fn check_cancelled(cancellation: &Cancellation) -> Result<(), Error> {
	if cancellation.is_cancelled() {
		return Err(Error::Cancelled);
	}
	Ok(())
}

fn charge(bytes: &mut usize, additional: usize) -> Result<(), Error> {
	*bytes = bytes.checked_sub(additional).ok_or(Error::MetadataBudget)?;
	Ok(())
}

fn charge_many<T>(bytes: &mut usize, count: usize) -> Result<(), Error> {
	charge(
		bytes,
		size_of::<T>()
			.checked_mul(count)
			.ok_or(Error::MetadataBudget)?,
	)
}
