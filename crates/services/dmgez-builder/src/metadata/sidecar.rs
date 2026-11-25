//! Explicit AppleDouble admission and native metadata conflict checking.

use std::{collections::TryReserveError, path::Path};

use dmgez_config::LocalPath;
use dmgez_core::source::{CapturedFile, SourceError};
use thiserror::Error;

use crate::manifest::{self, EntryKind, Manifest, SourceIdentity, SourceSnapshot};

use super::{Attribute, Attributes};

mod appledouble;

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests;

const APPLEDOUBLE_PREFIX: &[u8] = b"._";

/// A declared sidecar could not be bound, read, or reconciled with native metadata.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
	#[error("AppleDouble sidecar must be the explicit ._ companion of exactly one regular source")]
	Target,
	#[error("AppleDouble sidecar is also selected as image content")]
	Included,
	#[error("AppleDouble requires a source without hard links or a custom icon")]
	SharedTarget,
	#[error("could not capture the declared AppleDouble sidecar")]
	Manifest(#[from] manifest::Error),
	#[error("could not read the admitted AppleDouble sidecar")]
	Source(#[from] SourceError),
	#[error("AppleDouble sidecar exceeds the available metadata budget")]
	Budget,
	#[error("could not allocate bounded sidecar metadata")]
	Allocation(#[from] TryReserveError),
	#[error("malformed AppleDouble {0}")]
	Malformed(&'static str),
	#[error("unsupported AppleDouble {0}")]
	Unsupported(&'static str),
	#[error("AppleDouble sidecar disagrees with native {0}")]
	Conflict(Field),
	#[error("cannot compare the AppleDouble sidecar with native source attributes")]
	NativeUnavailable,
}

/// The native metadata field affected by a sidecar conflict.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[non_exhaustive]
pub enum Field {
	#[error("Finder information")]
	FinderInfo,
	#[error("resource fork")]
	ResourceFork,
	#[error("extended attribute")]
	ExtendedAttribute,
}

#[derive(Debug)]
pub(crate) struct Binding {
	auxiliary: usize,
	target: SourceIdentity,
}

impl Binding {
	pub(crate) fn resolve(
		manifest: &mut Manifest,
		input: &LocalPath,
		base: &Path,
		output: Option<&LocalPath>,
		max_metadata_bytes: u64,
	) -> Result<Self, Error> {
		let auxiliary =
			manifest.capture_auxiliary_regular(input, base, output, max_metadata_bytes)?;
		let snapshot = manifest.auxiliary_source(auxiliary);
		let sidecar = snapshot.path();
		let target_name = sidecar
			.file_name()
			.and_then(|name| name.as_encoded_bytes().strip_prefix(APPLEDOUBLE_PREFIX))
			.filter(|name| !name.is_empty())
			.ok_or(Error::Target)?;
		let mut target = None;
		for entry in manifest.entries() {
			let Some(source) = &entry.source else {
				continue;
			};
			if source.metadata().identity == snapshot.metadata().identity {
				return Err(Error::Included);
			}
			if matches!(entry.kind, EntryKind::File)
				&& source.path().parent() == sidecar.parent()
				&& source
					.path()
					.file_name()
					.map(|name| name.as_encoded_bytes())
					== Some(target_name)
			{
				if target.is_some() {
					return Err(Error::Target);
				}
				if source.metadata().link_count != 1 || entry.finder.custom_icon.is_some() {
					return Err(Error::SharedTarget);
				}
				target = Some(source.metadata().identity);
			}
		}
		Ok(Self {
			auxiliary,
			target: target.ok_or(Error::Target)?,
		})
	}

	pub(crate) fn sidecar_for<'a>(
		&self,
		source: &SourceSnapshot,
		manifest: &'a Manifest,
	) -> Option<&'a SourceSnapshot> {
		(self.target == source.metadata().identity)
			.then(|| manifest.auxiliary_source(self.auxiliary))
	}
}

/// Merges an explicitly bound AppleDouble file into observed native attributes.
///
/// Equal values are retained once. Different values are conflicts, including
/// when a later metadata policy would omit the affected attribute. Native-only
/// attributes are retained. The caller charges native capture separately.
///
/// # Errors
/// Returns an error for an unavailable native capture, a changed or invalid
/// sidecar, conflicting values, or exceeded aggregate metadata bounds. The byte
/// budget remains unchanged on error.
pub fn merge(
	native: super::policy::AttributeCapture,
	snapshot: &SourceSnapshot,
	remaining: &mut usize,
) -> Result<super::policy::AttributeCapture, Error> {
	let super::policy::AttributeCapture::Known(native) = native else {
		return Err(Error::NativeUnavailable);
	};
	let length = usize::try_from(snapshot.metadata().size).map_err(|_| Error::Budget)?;
	let working = subtract(*remaining, length)?;
	let mut bytes = Vec::new();
	bytes.try_reserve_exact(length)?;
	bytes.resize(length, 0);
	let file = CapturedFile::new(snapshot.open_regular()?)?;
	file.read_exact_at(0, &mut bytes)?;
	snapshot.validate()?;
	let parsed = appledouble::parse(&bytes, working)?;
	let (native, added) = merge_attributes(native, parsed, working)?;
	*remaining = subtract(*remaining, added)?;
	Ok(super::policy::AttributeCapture::Known(native))
}

fn merge_attributes(
	mut native: Attributes,
	sidecar: appledouble::Metadata<'_>,
	working: usize,
) -> Result<(Attributes, usize), Error> {
	let available = subtract(working, sidecar.descriptor_bytes())?;
	let mut left = available;
	if let Some(finder_info) = sidecar.finder_info {
		if native
			.finder_info
			.is_some_and(|current| current != finder_info)
		{
			return Err(Error::Conflict(Field::FinderInfo));
		}
		native.finder_info = Some(finder_info);
	}
	if let Some(fork) = sidecar.resource_fork {
		match native.resource_fork.as_deref() {
			Some(current) if current != fork => return Err(Error::Conflict(Field::ResourceFork)),
			Some(_) => {}
			None => native.resource_fork = Some(owned(fork, &mut left)?),
		}
	}
	let mut additions = 0_usize;
	for attribute in &sidecar.extended {
		match native_attribute(&native.extended, attribute.name) {
			Some(current) if current != attribute.value => {
				return Err(Error::Conflict(Field::ExtendedAttribute));
			}
			Some(_) => {}
			None => additions += 1,
		}
	}
	let new_records = additions.saturating_sub(native.extended.capacity() - native.extended.len());
	left = subtract(
		left,
		new_records
			.checked_mul(size_of::<Attribute>())
			.ok_or(Error::Budget)?,
	)?;
	native.extended.try_reserve_exact(additions)?;
	let before = native.extended.len();
	for attribute in sidecar.extended {
		if native_attribute(&native.extended[..before], attribute.name).is_none() {
			native.extended.push(Attribute {
				name: owned(attribute.name, &mut left)?,
				value: owned(attribute.value, &mut left)?,
			});
		}
	}
	native.extended.sort_unstable_by(|a, b| a.name.cmp(&b.name));
	let added = available - left;
	native.accounted_bytes = native
		.accounted_bytes
		.checked_add(added)
		.ok_or(Error::Budget)?;
	Ok((native, added))
}

fn native_attribute<'a>(attributes: &'a [Attribute], name: &[u8]) -> Option<&'a [u8]> {
	let index = attributes
		.binary_search_by(|field| field.name.as_slice().cmp(name))
		.ok()?;
	Some(&attributes[index].value)
}

fn owned(value: &[u8], left: &mut usize) -> Result<Vec<u8>, Error> {
	*left = subtract(*left, value.len())?;
	let mut bytes = Vec::new();
	bytes.try_reserve_exact(value.len())?;
	bytes.extend_from_slice(value);
	Ok(bytes)
}

fn subtract(remaining: usize, bytes: usize) -> Result<usize, Error> {
	remaining.checked_sub(bytes).ok_or(Error::Budget)
}
