use std::error::Error;

use hfsplus_builder::volume::{EntryMetadata, VolumeFormat};
use hfsplus_core::names::NameCollation;
use hfsplus_format::{
	bsd,
	catalog::{
		EncodeValue, FLAG_HAS_ATTRIBUTES, FLAG_HAS_FOLDER_COUNT, FLAG_THREAD_EXISTS, Metadata,
	},
	hard_links::{Neighbors, Stub},
	volume::{CLEAN_UNMOUNT_ATTRIBUTE, CatalogId, Fork, Header, Signature},
};
use hfsplus_reader::{
	catalog::{Entry, EntryKind, HardLink, HardLinkProfile},
	check::{AlternateStatus, AttributeCoverage, StructuralReport},
};

use super::CaptureError;

const PERMISSION_MASK: u16 = 0o7777;
const FINDER_ALIAS_FILE_TYPE: &[u8; 4] = b"alis";
const FINDER_FLAGS_OFFSET: usize = 8;
const FINDER_ALIAS_FLAG: u16 = 0x8000;
const VOLUME_FINDER_CATALOG_WORDS: usize = 6;
const VOLUME_FINDER_CATALOG_BYTES: usize = VOLUME_FINDER_CATALOG_WORDS * size_of::<u32>();
const MAC_ROMAN_ENCODING: u32 = 0;
const DEFAULT_ENCODING_BITMAP: u64 = 1 << MAC_ROMAN_ENCODING;
const HARD_LINK_CHAIN: u16 = 0x0020;
const NO_LINK_ID: u32 = 0;

pub(super) fn volume_format<E: Error + 'static>(
	header: &Header,
	collation: NameCollation,
) -> Result<VolumeFormat, CaptureError<E>> {
	let format = match (header.signature, collation) {
		(Signature::HfsPlus, NameCollation::CaseFolded) => VolumeFormat::HfsPlus,
		(Signature::HfsX, NameCollation::Binary) => VolumeFormat::HfsX,
		_ => return Err(CaptureError::VolumeProfile),
	};
	if header.attributes != CLEAN_UNMOUNT_ATTRIBUTE
		|| header.journal_info.is_some()
		|| header.forks.startup != Fork::empty()
		|| header.encodings & !DEFAULT_ENCODING_BITMAP != 0
	{
		return Err(CaptureError::VolumeProfile);
	}
	if header.finder_info[..VOLUME_FINDER_CATALOG_BYTES]
		.iter()
		.any(|&byte| byte != 0)
	{
		return Err(CaptureError::VolumeFinderReferences);
	}
	Ok(format)
}

pub(super) fn check_storage<E: Error + 'static>(
	report: &StructuralReport,
) -> Result<(), CaptureError<E>> {
	if report.alternate != AlternateStatus::MatchesPrimary
		|| report.unclaimed_allocated_blocks != 0
		|| report.unexamined_private_files != 0
		|| report.unsupported_directory_links != 0
		|| !matches!(
			report.attributes,
			AttributeCoverage::Absent
				| AttributeCoverage::DeclaredOwners {
					fork_backed: 0,
					unsupported: 0,
					..
				}
		) {
		return Err(CaptureError::StorageCoverage);
	}
	Ok(())
}

pub(super) fn directory<E: Error + 'static>(
	entry: &Entry,
	format: VolumeFormat,
) -> Result<EntryMetadata, CaptureError<E>> {
	let EntryKind::Directory { tail_word, .. } = entry.kind else {
		return Err(CaptureError::EntryMetadata(entry.metadata.id));
	};
	if entry.metadata.permissions.mode & bsd::TYPE_MASK != bsd::DIRECTORY {
		return Err(CaptureError::EntryMetadata(entry.metadata.id));
	}
	// The structural audit has checked HFSX counts before capture. Recompute
	// them when sealing because edits can change the immediate children.
	let flags = match format {
		VolumeFormat::HfsPlus if tail_word == 0 => FLAG_HAS_ATTRIBUTES,
		VolumeFormat::HfsX => FLAG_HAS_ATTRIBUTES | FLAG_HAS_FOLDER_COUNT,
		_ => return Err(CaptureError::EntryMetadata(entry.metadata.id)),
	};
	convert(&entry.metadata, flags, 0)
}

pub(super) fn symlink<E: Error + 'static>(
	raw: &Metadata,
	resource: &Fork,
) -> Result<EntryMetadata, CaptureError<E>> {
	// The rebuild would otherwise discard the resource fork's clump-size hint.
	if *resource != Fork::empty() {
		return Err(CaptureError::EntryKind(raw.id));
	}
	convert(raw, FLAG_THREAD_EXISTS | FLAG_HAS_ATTRIBUTES, 0)
}

pub(super) fn regular_file<E: Error + 'static>(
	raw: &Metadata,
	link: Option<HardLink>,
) -> Result<EntryMetadata, CaptureError<E>> {
	let flags = u16::from_be_bytes([
		raw.finder_info[FINDER_FLAGS_OFFSET],
		raw.finder_info[FINDER_FLAGS_OFFSET + 1],
	]);
	if flags & FINDER_ALIAS_FLAG != 0 || raw.finder_info.starts_with(FINDER_ALIAS_FILE_TYPE) {
		return Err(CaptureError::FinderReferences(raw.id));
	}
	let mut allowed_flags = FLAG_THREAD_EXISTS | FLAG_HAS_ATTRIBUTES;
	if link.is_some_and(|link| link.profile == HardLinkProfile::VerifiedChain) {
		// The reader validates the chain before admitting the shared inode.
		allowed_flags |= HARD_LINK_CHAIN;
	}
	convert(raw, allowed_flags, link.map_or(0, |link| link.count.get()))
}

pub(super) fn check_link_stub<E: Error + 'static>(
	raw: &Metadata,
	link: HardLink,
	created: u32,
) -> Result<(), CaptureError<E>> {
	let neighbors = match link.profile {
		HardLinkProfile::ChainFree => None,
		HardLinkProfile::VerifiedChain => Some(Neighbors {
			next: neighbor(raw.permissions.gid),
			previous: neighbor(raw.permissions.uid),
		}),
		_ => return Err(CaptureError::EntryMetadata(raw.id)),
	};
	let stub = Stub {
		reference: link.reference,
		neighbors,
	};
	let EncodeValue::File { metadata, .. } = stub
		.catalog_value(raw.id, created)
		.map_err(|_| CaptureError::EntryMetadata(raw.id))?
	else {
		return Err(CaptureError::EntryMetadata(raw.id));
	};
	// A generated stub contains structural pointers and canonical metadata.
	// The rebuild can regenerate those fields from the surviving link group.
	if *raw != metadata {
		return Err(CaptureError::EntryMetadata(raw.id));
	}
	Ok(())
}

fn neighbor(raw: u32) -> Option<CatalogId> {
	(raw != NO_LINK_ID).then_some(CatalogId(raw))
}

fn convert<E: Error + 'static>(
	raw: &Metadata,
	flags: u16,
	special: u32,
) -> Result<EntryMetadata, CaptureError<E>> {
	let permissions = raw.permissions;
	if raw.flags & !flags != 0
		|| raw.encoding != MAC_ROMAN_ENCODING
		|| permissions.admin_flags != 0
		|| permissions.owner_flags != 0
		|| permissions.special != special
	{
		return Err(CaptureError::EntryMetadata(raw.id));
	}
	Ok(EntryMetadata {
		uid: permissions.uid,
		gid: permissions.gid,
		mode: permissions.mode & PERMISSION_MASK,
		dates: raw.dates,
		finder_info: raw.finder_info,
	})
}
