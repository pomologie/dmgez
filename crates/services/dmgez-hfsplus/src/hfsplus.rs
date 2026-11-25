use std::{collections::TryReserveError, io::Write, mem::size_of, path::Path};

#[cfg(not(windows))]
use dmgez_builder::metadata as host_metadata;
use dmgez_builder::{
	execution::{BuildPlan, BuildReport, ExecuteError, OutputVerifier},
	metadata::{Attribute, policy, sidecar},
	planning::{self, PreparedBuild},
	preflight::{
		Availability, BuildCapabilities, ExecutionProfile, FilesystemProfile, PreflightError,
		PreflightOptions,
	},
	progress::BuildProgress,
};
use dmgez_config::{Capacity, Project, VirtualPath};
use dmgez_udif::writer::{Cancellation, WriteOptions};
use hfsplus_builder::{
	SealedPlan,
	volume::{self, CapacityError, CapacityPolicy, ManifestEntry, VolumeError, VolumeOptions},
};
use hfsplus_core::ByteLen;
use thiserror::Error;

#[cfg(feature = "finder-write")]
mod finder;
mod hard_links;
#[cfg(feature = "volume-icon")]
mod item_icons;
mod metadata;
mod release_stage;
mod source;

use metadata::{Capture, DEFAULT_FOLDER_MODE, MISSING_HFS_DATE, epoch_seconds, generated_metadata};
use source::Payload;

#[cfg(feature = "image-edit")]
pub use metadata::HostCopyMetadataLoss;
#[cfg(feature = "image-edit")]
pub(crate) use metadata::{HostCapture, HostEntry, HostKind, capture_host_copies};
#[cfg(feature = "image-edit")]
pub(crate) use source::read_host_source;

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
	#[cfg(feature = "finder-write")]
	#[error(transparent)]
	Finder(#[from] dmgez_builder::presentation::Error),
	#[cfg(feature = "finder-write")]
	#[error("a reserved Finder background identity is missing at {path:?}")]
	BackgroundIdentity { path: String },
	#[cfg(feature = "finder-write")]
	#[error("the stored HFS+ volume name cannot be represented in a Finder alias")]
	BackgroundVolumeName(#[source] std::string::FromUtf16Error),
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
	#[error("hard-link source validation failed")]
	LinkSource(#[source] dmgez_builder::manifest::Error),
	#[error("hard-link aliases have inconsistent source metadata at {path:?}")]
	LinkMetadata { path: String },
	#[cfg(not(windows))]
	#[error("could not capture regular-source metadata for {path:?}")]
	Capture {
		path: String,
		#[source]
		cause: host_metadata::Error,
	},
	#[error("could not merge the AppleDouble sidecar for {path:?}")]
	Sidecar {
		path: String,
		#[source]
		cause: sidecar::Error,
	},
	#[error("unsupported source kind or missing source at {path:?}")]
	UnsupportedEntry { path: String },
	#[error("requested Finder flag overrides are unavailable at {path:?}")]
	FinderFlags { path: String },
	#[cfg(feature = "volume-icon")]
	#[error("a custom Finder icon cannot be applied to the item at {path:?}")]
	CustomIconTarget { path: String },
	#[cfg(feature = "volume-icon")]
	#[error("a custom Finder icon cannot change shared hard-link metadata at {path:?}")]
	CustomIconHardLink { path: String },
	#[cfg(feature = "volume-icon")]
	#[error("a custom Finder icon would replace an existing resource fork at {path:?}")]
	CustomIconResourceFork { path: String },
	#[cfg(all(feature = "volume-icon", not(windows)))]
	#[error("a custom Finder icon would invalidate native code-signing metadata at {path:?}")]
	CustomIconSigningMetadata { path: String },
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
	/// The captured image path, or the representative path for a shared inode.
	pub path: &'build VirtualPath,
	/// Other catalog paths backed by that same inode.
	pub aliases: Vec<&'build VirtualPath>,
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
	if cfg!(any(target_os = "linux", target_os = "macos")) {
		capabilities.metadata_sidecar = Availability::Available;
	}
	capabilities.atomic_replace = if cfg!(windows) {
		Availability::Unavailable
	} else {
		Availability::Available
	};
	if cfg!(feature = "finder-write") {
		capabilities.finder_layout = Availability::Available;
		capabilities.finder_image_background = Availability::Available;
	}
	if cfg!(feature = "volume-icon") {
		capabilities.finder_volume_icon = Availability::Available;
		capabilities.finder_custom_icon = if cfg!(windows) {
			Availability::Unavailable
		} else {
			Availability::Available
		};
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
	prepare_with_progress(project, config_dir, options, cancellation, |_| {})
}

/// Admit the build and report shared planning progress on the calling thread.
///
/// # Errors
/// Returns the same errors as [`prepare`].
pub fn prepare_with_progress<'project>(
	project: &'project Project,
	config_dir: &Path,
	options: PreflightOptions,
	cancellation: Cancellation,
	progress: impl Fn(BuildProgress),
) -> Result<PreparedHfsBuild<'project>, Error> {
	let build =
		planning::prepare_with_progress(project, config_dir, options, cancellation, progress)?;
	Ok(PreparedHfsBuild { project, build })
}

impl PreparedHfsBuild<'_> {
	/// The frozen source manifest, capabilities selected by preflight, and destination.
	#[must_use]
	pub const fn prepared(&self) -> &PreparedBuild {
		&self.build
	}

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
		self.publish_with_progress(verifier, |_| {})
	}

	/// Publish the validated image and report shared execution progress.
	///
	/// # Errors
	/// Returns the same errors as [`Self::publish`].
	pub fn publish_with_progress(
		self,
		verifier: Option<&OutputVerifier<'_>>,
		progress: impl Fn(BuildProgress),
	) -> Result<HfsBuildReport<'build>, Error> {
		let output = self.prepared.output_path().ok_or(Error::OutputPath)?;
		let build = BuildPlan::new(
			self.prepared.profile(),
			self.prepared.manifest(),
			self.image.reader(),
			self.logical_bytes(),
		)?;
		let report = build.publish_with_progress(output, verifier, progress)?;
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
	let supports_file_hard_links = profile.filesystem() == FilesystemProfile::HfsplusUnicode;
	let shared_member = |entry| {
		if supports_file_hard_links && project.metadata.preserve_hard_links {
			hard_links::Member::new(entry, &links)
		} else {
			None
		}
	};
	let shared_count = manifest.entries().iter().filter_map(shared_member).count();
	charge_many::<hard_links::Member<'_>>(&mut remaining, shared_count)?;
	let mut members = Vec::new();
	members.try_reserve_exact(shared_count)?;

	let count = manifest.entries().len() - shared_count;
	#[cfg(feature = "finder-write")]
	let count = count
		.checked_add(finder::account_assets(prepared, &mut remaining)?)
		.ok_or(Error::MetadataBudget)?;
	#[cfg(feature = "volume-icon")]
	let count = count
		.checked_add(item_icons::account_assets(prepared, &mut remaining)?)
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
		requests: &project.metadata,
		supports_file_hard_links,
		remaining: &mut remaining,
	};
	for entry in manifest.entries() {
		check_cancelled(cancellation)?;
		if let Some(member) = shared_member(entry) {
			#[cfg(feature = "volume-icon")]
			item_icons::reject_shared(entry)?;
			members.push(member);
			continue;
		}
		let captured = capture.entry(entry, generated_seconds, prepared)?;
		let catalog_entry = captured.entry;
		if !captured.losses.is_empty() {
			losses.push(MetadataLoss {
				path: &entry.destination,
				aliases: Vec::new(),
				details: captured.losses,
			});
		}
		#[cfg(feature = "volume-icon")]
		let catalog_entry = item_icons::apply(
			prepared,
			entry,
			catalog_entry,
			&mut entries,
			generated_seconds,
		)?;
		if !captured.attributes.is_empty() {
			pending_attrs.push((catalog_entry.path, captured.attributes));
		}
		entries.push(catalog_entry);
	}
	let groups = hard_links::assemble(
		members,
		&mut capture,
		&mut pending_attrs,
		&mut losses,
		generated_seconds,
		cancellation,
		prepared,
	)?;
	#[cfg(feature = "finder-write")]
	finder::reserve_assets(prepared, &mut entries, generated_seconds, cancellation)?;

	let mut options =
		VolumeOptions::for_capacity(project.image.name.as_str(), ALLOCATION_BLOCK_BYTES)?;
	options.max_entries = profile.limits().max_entries;
	options.limits.max_metadata_bytes =
		ByteLen(u64::try_from(remaining).map_err(|_| Error::MetadataBudget)?);
	options.root = generated_metadata(DEFAULT_FOLDER_MODE, generated_seconds);
	#[cfg(feature = "volume-icon")]
	prepared.apply_root_finder_info(&mut options.root.finder_info);
	options.dates = [
		generated_seconds,
		generated_seconds,
		MISSING_HFS_DATE,
		MISSING_HFS_DATE,
	];

	let mut draft = match profile.filesystem() {
		FilesystemProfile::HfsplusUnicode => {
			volume::plan_with_hard_links(options, entries, groups)?
		}
		FilesystemProfile::HfsplusAscii => volume::plan_ascii(options, entries)?,
		_ => return Err(Error::UnsupportedProfile),
	};
	#[cfg(feature = "finder-write")]
	finder::bind_assets(
		prepared,
		&mut draft,
		project.image.name.as_str(),
		generated_seconds,
		cancellation,
	)?;
	for (path, attrs) in pending_attrs {
		for attr in attrs {
			check_cancelled(cancellation)?;
			draft.add_inline_attribute(path, std::str::from_utf8(&attr.name)?, &attr.value)?;
		}
	}
	let image = draft.seal_with_capacity(capacity(profile)?)?;
	profile.validate_logical_bytes(image.image_len().0)?;
	check_cancelled(cancellation)?;
	losses.sort_unstable_by(|left, right| left.path.as_str().cmp(right.path.as_str()));
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
