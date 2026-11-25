use std::{borrow::Cow, mem::size_of};

use dmgez_builder::{
	manifest,
	metadata::{self, Attribute, AttributeLimits, policy},
};
use dmgez_config::Project;
use hfsplus_builder::volume::{self, EntryKind, EntryMetadata, ForkSource, ManifestEntry};

use super::{Error, MetadataLoss, Payload, charge, relative};

const HFS_EPOCH_SECONDS: i64 = 2_082_844_800;
const FIRST_HFS_UNIX_SECOND: i64 = 1 - HFS_EPOCH_SECONDS;
const LAST_HFS_UNIX_SECOND: i64 = u32::MAX as i64 - HFS_EPOCH_SECONDS;
const POSIX_PERMISSION_MASK: u16 = 0o7777;
pub(super) const DEFAULT_FILE_MODE: u16 = 0o644;
pub(super) const DEFAULT_FOLDER_MODE: u16 = 0o755;
const DEFAULT_SYMLINK_MODE: u16 = 0o777;
pub(super) const MISSING_HFS_DATE: u32 = 0;

pub(super) struct Capture<'scope, 'config, 'build> {
	pub(super) policy: &'scope policy::Policy<'config>,
	pub(super) links: &'scope policy::HardLinks,
	pub(super) project: &'config Project,
	pub(super) supports_file_hard_links: bool,
	pub(super) remaining: &'scope mut usize,
	pub(super) losses: &'scope mut Vec<MetadataLoss<'build>>,
}

impl<'build> Capture<'_, '_, 'build> {
	pub(super) fn entry(
		&mut self,
		entry: &'build manifest::Entry,
		generated_seconds: u32,
	) -> Result<(ManifestEntry<'build, Payload<'build>>, Vec<Attribute>), Error> {
		let path = relative(&entry.destination)?;
		validate_finder_overrides(entry)?;
		let unsupported_entry = || Error::UnsupportedEntry {
			path: entry.destination.as_str().into(),
		};
		let mode = match &entry.kind {
			manifest::EntryKind::Directory => DEFAULT_FOLDER_MODE,
			manifest::EntryKind::File => DEFAULT_FILE_MODE,
			manifest::EntryKind::Symlink(_) => DEFAULT_SYMLINK_MODE,
			_ => return Err(unsupported_entry()),
		};
		let mut metadata = generated_metadata(mode, generated_seconds);
		let mut fork = None;
		let mut extended = Vec::new();
		if let Some(source) = &entry.source {
			let is_file = matches!(&entry.kind, manifest::EntryKind::File);
			let attributes = self.capture_attributes(entry, source, is_file)?;
			let mut admitted = self
				.policy
				.admit(
					source.metadata(),
					attributes,
					self.links.group_for(entry),
					&metadata_capabilities(is_file, self.supports_file_hard_links),
					self.remaining,
				)
				.map_err(|cause| Error::Policy {
					path: entry.destination.as_str().into(),
					cause,
				})?;
			if !admitted.losses.is_empty() {
				self.losses.push(MetadataLoss {
					path: &entry.destination,
					aliases: Vec::new(),
					details: admitted.losses,
				});
			}

			metadata = entry_metadata(
				mode,
				admitted.metadata.permissions,
				admitted.metadata.ownership,
				admitted.metadata.finder_info,
				&admitted.metadata,
			)?;
			fork = admitted
				.metadata
				.resource_fork
				.take()
				.map(|bytes| ForkSource::whole(Payload::Bytes(Cow::Owned(bytes))));
			extended = admitted.metadata.extended_attributes;
		}

		let kind = match &entry.kind {
			manifest::EntryKind::Directory => EntryKind::Folder,
			manifest::EntryKind::File => EntryKind::File {
				data: ForkSource::whole(Payload::File(
					entry.source.as_ref().ok_or_else(unsupported_entry)?,
				)),
				resource: fork,
			},
			manifest::EntryKind::Symlink(target) => {
				charge(self.remaining, target.len())?;
				let mut copied = Vec::new();
				copied.try_reserve_exact(target.len())?;
				copied.extend_from_slice(target);
				EntryKind::Symlink { target: copied }
			}
			_ => return Err(unsupported_entry()),
		};
		Ok((
			ManifestEntry {
				path,
				metadata,
				kind,
			},
			extended,
		))
	}

	fn capture_attributes(
		&mut self,
		entry: &manifest::Entry,
		source: &manifest::SourceSnapshot,
		is_file: bool,
	) -> Result<policy::AttributeCapture, Error> {
		let requests = &self.project.metadata;
		#[cfg(feature = "volume-icon")]
		if is_file && entry.finder.custom_icon.is_some() {
			validate_icon_source(entry, source)?;
		}
		if !(requests.preserve_finder_info
			|| requests.preserve_resource_forks
			|| requests.preserve_extended_attributes)
		{
			return Ok(policy::AttributeCapture::NotApplicable);
		}
		if !is_file {
			return Ok(policy::AttributeCapture::Unavailable);
		}
		let (_, attributes) = metadata::capture_regular(
			source,
			AttributeLimits {
				bytes: *self.remaining,
				value_bytes: *self.remaining,
				attributes: *self.remaining / size_of::<Attribute>(),
			},
		)
		.map_err(|cause| Error::Capture {
			path: entry.destination.as_str().into(),
			cause,
		})?;
		charge(self.remaining, attributes.accounted_bytes)?;
		Ok(policy::AttributeCapture::Known(attributes))
	}
}

#[cfg(feature = "volume-icon")]
fn validate_icon_source(
	entry: &manifest::Entry,
	source: &manifest::SourceSnapshot,
) -> Result<(), Error> {
	use metadata::ItemIconSourceConflict;

	let conflict = metadata::probe_item_icon_conflict(source).map_err(|cause| Error::Capture {
		path: entry.destination.as_str().into(),
		cause,
	})?;
	match conflict {
		Some(ItemIconSourceConflict::ResourceFork) => Err(Error::CustomIconResourceFork {
			path: entry.destination.as_str().into(),
		}),
		Some(ItemIconSourceConflict::SigningMetadata) => Err(Error::CustomIconSigningMetadata {
			path: entry.destination.as_str().into(),
		}),
		None => Ok(()),
	}
}

pub(super) fn validate_finder_overrides(entry: &manifest::Entry) -> Result<(), Error> {
	if entry.finder.hidden.is_some()
		|| entry.finder.hide_extension.is_some()
		|| (!cfg!(feature = "volume-icon") && entry.finder.custom_icon.is_some())
	{
		return Err(Error::FinderFlags {
			path: entry.destination.as_str().into(),
		});
	}
	Ok(())
}

fn metadata_capabilities(is_file: bool, supports_file_hard_links: bool) -> policy::Capabilities {
	policy::Capabilities {
		permissions: true,
		ownership: true,
		timestamps: Some(policy::TimestampEncoding {
			unix_seconds: FIRST_HFS_UNIX_SECOND..=LAST_HFS_UNIX_SECOND,
			precision: policy::TimestampPrecision::Seconds,
		}),
		finder_info: true,
		max_resource_fork_bytes: is_file.then_some(u64::MAX),
		extended_attributes: Some(can_inline),
		hard_links: is_file && supports_file_hard_links,
		..policy::Capabilities::default()
	}
}

fn can_inline(name: &[u8], bytes: usize) -> bool {
	std::str::from_utf8(name)
		.is_ok_and(|name| volume::validate_inline_attribute(name, bytes).is_ok())
}

fn entry_metadata(
	mode: u16,
	permissions: Option<u16>,
	ownership: Option<(u32, u32)>,
	finder: Option<[u8; 32]>,
	times: &policy::AdmittedMetadata,
) -> Result<EntryMetadata, Error> {
	let mut result = EntryMetadata::new(permissions.unwrap_or(mode) & POSIX_PERMISSION_MASK);
	if let Some((uid, gid)) = ownership {
		result.uid = uid;
		result.gid = gid;
	}
	if let Some(info) = finder {
		result.finder_info = info;
	}
	for (field, time) in
		result
			.dates
			.iter_mut()
			.zip([times.created, times.modified, times.changed, times.accessed])
	{
		*field = time
			.map(|time| hfs_seconds(time.seconds))
			.transpose()?
			.unwrap_or(MISSING_HFS_DATE);
	}
	Ok(result)
}

pub(super) fn generated_metadata(mode: u16, date: u32) -> EntryMetadata {
	let mut metadata = EntryMetadata::new(mode);
	metadata.dates = [date, date, date, date, MISSING_HFS_DATE];
	metadata
}

pub(super) fn epoch_seconds(seconds: u64) -> Result<u32, Error> {
	hfs_seconds(i64::try_from(seconds).map_err(|_| Error::Timestamp)?)
}

fn hfs_seconds(seconds: i64) -> Result<u32, Error> {
	if !(FIRST_HFS_UNIX_SECOND..=LAST_HFS_UNIX_SECOND).contains(&seconds) {
		return Err(Error::Timestamp);
	}
	u32::try_from(seconds + HFS_EPOCH_SECONDS).map_err(|_| Error::Timestamp)
}
