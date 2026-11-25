use dmgez_builder::manifest::Entry;
#[cfg(target_os = "macos")]
use dmgez_builder::manifest::EntryKind;
use dmgez_builder::presentation::signed_targets::SignedTargets;
use dmgez_finder::finder_info::DisplayFlags;

use super::{Error, charge};

pub(super) fn overrides(entry: &Entry) -> DisplayFlags {
	DisplayFlags {
		hidden: entry.finder.hidden,
		hide_extension: entry.finder.hide_extension,
	}
}

pub(super) fn signed_targets<'entries>(
	entries: &'entries [Entry],
	remaining: &mut usize,
) -> Result<Option<SignedTargets<'entries>>, Error> {
	if !entries
		.iter()
		.any(|entry| overrides(entry) != DisplayFlags::default())
	{
		return Ok(None);
	}
	let targets = SignedTargets::new(entries, *remaining).map_err(Error::FinderFlagTargetIndex)?;
	charge(remaining, targets.metadata_bytes())?;
	Ok(Some(targets))
}

pub(super) fn validate_target(targets: &SignedTargets<'_>, entry: &Entry) -> Result<(), Error> {
	if let Some(protection) =
		targets
			.protection(entry)
			.map_err(|source| Error::FinderFlagTarget {
				path: entry.destination.as_str().into(),
				source,
			})? {
		return Err(Error::FinderFlagsProtected {
			path: entry.destination.as_str().into(),
			protection,
		});
	}
	#[cfg(target_os = "macos")]
	if matches!(entry.kind, EntryKind::File)
		&& let Some(source) = &entry.source
		&& dmgez_builder::metadata::has_native_signing_attributes(source).map_err(|cause| {
			Error::Capture {
				path: entry.destination.as_str().into(),
				cause,
			}
		})? {
		return Err(signing_attributes(entry));
	}
	Ok(())
}

pub(super) fn signing_attributes(entry: &Entry) -> Error {
	Error::FinderFlagsSigningMetadata {
		path: entry.destination.as_str().into(),
	}
}
