//! Capture host additions with the ordinary HFS+ build policy.

use dmgez_builder::{
	manifest::{Entry, EntryKind, Manifest, SourceIdentity, SourceSnapshot},
	metadata::{Attribute, policy},
	planning,
};
use dmgez_config::{Metadata, MetadataPolicy, VirtualPath};
use hfsplus_builder::volume::EntryMetadata;

use super::{Capture, Error, MISSING_HFS_DATE, copy_symlink, unsupported};
use crate::hfsplus::{charge, charge_many, hard_links};

pub(crate) struct HostCapture {
	pub(crate) entries: Vec<HostEntry>,
	pub(crate) losses: Vec<HostCopyMetadataLoss>,
	pub(crate) accounted_bytes: usize,
}

pub(crate) struct HostEntry {
	pub(crate) path: VirtualPath,
	pub(crate) aliases: Vec<VirtualPath>,
	pub(crate) metadata: EntryMetadata,
	pub(crate) kind: HostKind,
	pub(crate) attributes: Vec<Attribute>,
}

pub(crate) enum HostKind {
	Directory,
	File {
		source: Box<SourceSnapshot>,
		resource: Option<Vec<u8>>,
		identity: Option<SourceIdentity>,
	},
	Symlink(Vec<u8>),
}

/// Metadata omissions encountered while importing a host entry into an image.
#[derive(Debug)]
pub struct HostCopyMetadataLoss {
	/// Destination of the imported entry or representative hard link.
	pub path: VirtualPath,
	/// Other destinations referring to the same retained regular file.
	pub aliases: Vec<VirtualPath>,
	/// Source fields that could not be retained by the selected profile.
	pub details: Vec<policy::Loss>,
}

pub(crate) fn capture_host_copies(
	manifest: &Manifest,
	policy: MetadataPolicy,
	max_metadata_bytes: usize,
) -> Result<HostCapture, Error> {
	let mut remaining = max_metadata_bytes;
	charge(
		&mut remaining,
		usize::try_from(manifest.metadata_bytes()).map_err(|_| Error::MetadataBudget)?,
	)?;
	let links = policy::HardLinks::discover(manifest, remaining).map_err(Error::HardLinks)?;
	charge(&mut remaining, links.accounted_bytes())?;
	let sources = || {
		manifest
			.entries()
			.iter()
			.filter(|entry| entry.source.is_some())
	};
	let group_members = sources()
		.filter_map(|entry| hard_links::Member::new(entry, &links))
		.count();
	charge_many::<hard_links::Member<'_>>(&mut remaining, group_members)?;
	let mut members = Vec::new();
	members.try_reserve_exact(group_members)?;

	let count = sources().count();
	charge_many::<HostEntry>(&mut remaining, count)?;
	let mut entries = Vec::new();
	entries.try_reserve_exact(count)?;
	charge_many::<HostCopyMetadataLoss>(&mut remaining, count)?;
	let mut losses = Vec::new();
	losses.try_reserve_exact(count)?;
	let mut requests = Metadata::default();
	requests.policy = policy;
	let policy = policy::Policy::new(&requests, policy == MetadataPolicy::Strict);
	let mut capture = Capture {
		policy: &policy,
		links: &links,
		requests: &requests,
		supports_file_hard_links: true,
		remaining: &mut remaining,
	};
	for entry in sources() {
		if let Some(member) = hard_links::Member::new(entry, &links) {
			members.push(member);
		} else {
			entries.push(capture_entry(&mut capture, entry, None, &[], &mut losses)?);
		}
	}
	hard_links::sort_members(&mut members);
	for group in members.chunk_by(|left, right| left.identity == right.identity) {
		let representative = &group[0];
		for alias in &group[1..] {
			hard_links::validate_alias(representative.entry, alias.entry)?;
		}
		entries.push(capture_entry(
			&mut capture,
			representative.entry,
			Some(representative.identity),
			&group[1..],
			&mut losses,
		)?);
	}
	entries.sort_unstable_by(|left, right| left.path.cmp(&right.path));
	losses.sort_unstable_by(|left, right| left.path.cmp(&right.path));
	Ok(HostCapture {
		entries,
		losses,
		accounted_bytes: max_metadata_bytes - remaining,
	})
}

fn capture_entry(
	capture: &mut Capture<'_, '_>,
	entry: &Entry,
	identity: Option<SourceIdentity>,
	aliases: &[hard_links::Member<'_>],
	losses: &mut Vec<HostCopyMetadataLoss>,
) -> Result<HostEntry, Error> {
	let source = entry.source.as_ref().ok_or_else(|| unsupported(entry))?;
	source.validate().map_err(planning::Error::from)?;
	let captured = capture.metadata(entry, MISSING_HFS_DATE, None)?;
	let kind = match &entry.kind {
		EntryKind::Directory => HostKind::Directory,
		EntryKind::File => {
			charge_many::<SourceSnapshot>(capture.remaining, 1)?;
			charge(capture.remaining, source.path().as_os_str().len())?;
			HostKind::File {
				source: Box::new(source.clone()),
				resource: captured.resource,
				identity,
			}
		}
		EntryKind::Symlink(target) => HostKind::Symlink(copy_symlink(target, capture.remaining)?),
		_ => return Err(unsupported(entry)),
	};
	let path = clone_path(capture.remaining, &entry.destination)?;
	let aliases = copy_aliases(
		capture.remaining,
		aliases.iter().map(|member| &member.entry.destination),
	)?;
	if !captured.losses.is_empty() {
		losses.push(HostCopyMetadataLoss {
			path: clone_path(capture.remaining, &path)?,
			aliases: copy_aliases(capture.remaining, aliases.iter())?,
			details: captured.losses,
		});
	}
	Ok(HostEntry {
		path,
		aliases,
		metadata: captured.metadata,
		kind,
		attributes: captured.attributes,
	})
}

fn copy_aliases<'path>(
	remaining: &mut usize,
	paths: impl ExactSizeIterator<Item = &'path VirtualPath>,
) -> Result<Vec<VirtualPath>, Error> {
	charge_many::<VirtualPath>(remaining, paths.len())?;
	let mut aliases = Vec::new();
	aliases.try_reserve_exact(paths.len())?;
	for path in paths {
		aliases.push(clone_path(remaining, path)?);
	}
	Ok(aliases)
}

fn clone_path(remaining: &mut usize, path: &VirtualPath) -> Result<VirtualPath, Error> {
	charge(remaining, path.as_str().len())?;
	Ok(path.clone())
}
