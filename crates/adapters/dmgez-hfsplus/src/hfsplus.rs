use std::{io::Write, mem::size_of, path::Path};

use dmgez_builder::{
	execution::{BuildPlan, BuildReport, OutputVerifier},
	metadata::{Attribute, policy},
	partition::Envelope,
	planning::{self, PreparedBuild},
	preflight::{
		Availability, BuildCapabilities, ExecutionProfile, FilesystemProfile, PreflightError,
		PreflightOptions,
	},
	progress::BuildProgress,
};
#[cfg(feature = "finder-write")]
use dmgez_config::Filesystem;
use dmgez_config::{Capacity, Project, VirtualPath};
use dmgez_udif::writer::Cancellation;
use hfsplus_builder::{
	SealedPlan,
	volume::{self, CapacityPolicy, ManifestEntry, VolumeFormat, VolumeOptions},
};
use hfsplus_core::ByteLen;

mod error;
#[cfg(feature = "finder-write")]
mod finder;
#[cfg(feature = "finder-write")]
mod finder_flags;
mod hard_links;
#[cfg(feature = "volume-icon")]
mod item_icons;
mod metadata;
mod release_stage;
mod source;

pub use error::Error;
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
	logical_bytes: u64,
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
	capabilities.hfsx = Availability::Available;
	if cfg!(any(target_os = "linux", target_os = "macos")) {
		capabilities.metadata_sidecar = Availability::Available;
	}
	capabilities.atomic_replace = if cfg!(windows) {
		Availability::Unavailable
	} else {
		Availability::Available
	};
	if cfg!(feature = "finder-write") {
		capabilities.finder_flags = Availability::Available;
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
	cancel: Cancellation,
	progress: impl Fn(BuildProgress),
) -> Result<PreparedHfsBuild<'project>, Error> {
	#[cfg(feature = "finder-write")]
	let build = planning::prepare_for_catalog(
		project,
		config_dir,
		options,
		cancel,
		progress,
		if project.image.filesystem == Filesystem::Hfsx {
			finder::HFSX_CATALOG_NAMES
		} else {
			finder::CATALOG_NAMES
		},
	)?;
	#[cfg(not(feature = "finder-write"))]
	let build = planning::prepare_with_progress(project, config_dir, options, cancel, progress)?;
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
	/// Exact logical disk length, including any partition map and alignment padding.
	#[must_use]
	pub const fn logical_bytes(&self) -> u64 {
		self.logical_bytes
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
		let envelope = self.envelope()?;
		let build = BuildPlan::from_prepared(
			self.prepared,
			envelope.reader(self.image.reader()),
			self.logical_bytes(),
		)?;
		let report = build.publish_with_progress(output, verifier, progress)?;
		Ok(HfsBuildReport {
			report,
			metadata_losses: self.metadata_losses,
		})
	}

	/// Write the selected image format to the admitted sink without an intermediate file.
	///
	/// # Errors
	/// Fails on invalid output intent, source mutation, cancellation, or I/O.
	pub fn write_to(self, sink: impl Write) -> Result<HfsBuildReport<'build>, Error> {
		let envelope = self.envelope()?;
		let build = BuildPlan::from_prepared(
			self.prepared,
			envelope.reader(self.image.reader()),
			self.logical_bytes(),
		)?;
		let report = build.write_to(sink)?;
		Ok(HfsBuildReport {
			report,
			metadata_losses: self.metadata_losses,
		})
	}

	fn envelope(&self) -> Result<Envelope, Error> {
		let profile = self.prepared.profile();
		Ok(Envelope::from_volume_with_filesystem(
			profile.partition(),
			profile.filesystem(),
			self.image.image_len().0,
			|| self.image.reader(),
			&profile.write_options().cancellation,
		)?)
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
	#[cfg(feature = "finder-write")]
	let finder_flag_targets = finder_flags::signed_targets(manifest.entries(), &mut remaining)?;
	let links = policy::HardLinks::discover(manifest, remaining).map_err(Error::HardLinks)?;
	charge(&mut remaining, links.accounted_bytes())?;
	let supports_file_hard_links = matches!(
		profile.filesystem(),
		FilesystemProfile::HfsplusUnicode | FilesystemProfile::Hfsx
	);
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
		#[cfg(feature = "finder-write")]
		let flags = {
			let flags = finder_flags::overrides(entry);
			if flags != Default::default()
				&& let Some(targets) = &finder_flag_targets
			{
				finder_flags::validate_target(targets, entry)?;
			}
			flags
		};
		let captured = capture.entry(
			entry,
			generated_seconds,
			prepared,
			#[cfg(feature = "finder-write")]
			flags,
		)?;
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
		prepared,
		#[cfg(feature = "finder-write")]
		finder_flag_targets.as_ref(),
	)?;
	#[cfg(feature = "finder-write")]
	finder::reserve_assets(prepared, &mut entries, generated_seconds, cancellation)?;

	let mut options =
		VolumeOptions::for_capacity(project.image.name.as_str(), ALLOCATION_BLOCK_BYTES)?;
	if profile.filesystem() == FilesystemProfile::Hfsx {
		options.format = VolumeFormat::HfsX;
	}
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
		FilesystemProfile::HfsplusUnicode | FilesystemProfile::Hfsx => {
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
	let logical_bytes = profile
		.partition()
		.disk_bytes(image.image_len().0)
		.ok_or(PreflightError::PartitionGeometry)?;
	profile.validate_logical_bytes(logical_bytes)?;
	check_cancelled(cancellation)?;
	losses.sort_unstable_by(|left, right| left.path.as_str().cmp(right.path.as_str()));
	Ok(HfsImage {
		prepared,
		image,
		logical_bytes,
		metadata_losses: losses,
	})
}

fn capacity(profile: &ExecutionProfile) -> Result<CapacityPolicy, Error> {
	let free_space = ByteLen(profile.free_space_bytes());
	let writer_max = profile.max_logical_bytes();
	let volume_bound = |disk_bytes: u64| {
		disk_bytes
			.checked_sub(profile.partition().overhead_bytes())
			.map(ByteLen)
			.ok_or(PreflightError::PartitionGeometry)
	};

	Ok(match profile.capacity() {
		Capacity::Auto { .. } => CapacityPolicy::ShrinkWithin {
			max_bytes: volume_bound(writer_max)?,
			free_space,
		},
		Capacity::Exact(bytes) => CapacityPolicy::Exact {
			bytes: volume_bound(bytes.get())?,
			free_space,
		},
		Capacity::ShrinkWithin(bytes) => CapacityPolicy::ShrinkWithin {
			max_bytes: volume_bound(bytes.get().min(writer_max))?,
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
