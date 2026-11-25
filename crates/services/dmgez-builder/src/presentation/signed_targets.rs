//! Identity-checked source protection for FinderInfo and resource fork changes.

use std::{collections::TryReserveError, mem::size_of};

use dmgez_core::source::{CapturedFile, SourceError};
use dmgez_macho::bundle::{
	has_app_suffix, inside_signed_bundle, signed_bundle_root, sort_signed_bundle_roots,
};
use thiserror::Error;

use crate::manifest::{self, Entry, EntryKind};

/// A recognized bundle boundary or a file that may have executable signature data.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum Protection {
	#[error("a protected application bundle or framework")]
	Bundle,
	#[error("a file with unverified executable signature integrity")]
	Executable,
}

/// A candidate's protection status could not be established.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
	#[error("bundle signature markers exceed the metadata budget")]
	Budget,
	#[error("cannot reserve bundle signature marker memory")]
	Reserve(#[from] TryReserveError),
	#[error("source identity check failed")]
	Source(#[from] manifest::Error),
	#[error("could not read captured source to check executable signature")]
	Read(#[from] SourceError),
}

/// Recognized bundle roots indexed from the immutable project manifest.
pub struct SignedTargets<'entries> {
	roots: Vec<&'entries str>,
}

impl<'entries> SignedTargets<'entries> {
	/// Indexes the destinations of conventional regular CodeResources markers.
	///
	/// # Errors
	/// Returns an error if the marker index cannot fit within the caller's budget.
	pub fn new(entries: &'entries [Entry], max_metadata_bytes: usize) -> Result<Self, Error> {
		let signatures = entries
			.iter()
			.filter(|entry| matches!(entry.kind, EntryKind::File))
			.filter_map(|entry| signed_bundle_root(entry.destination.as_str()));
		let mut roots = Vec::new();
		let count = signatures.clone().count();
		let check_budget = |capacity: usize| {
			capacity
				.checked_mul(size_of::<&str>())
				.filter(|&bytes| bytes <= max_metadata_bytes)
				.ok_or(Error::Budget)
		};
		check_budget(count)?;
		roots.try_reserve_exact(count)?;
		check_budget(roots.capacity())?;
		roots.extend(signatures);
		sort_signed_bundle_roots(&mut roots);
		Ok(Self { roots })
	}

	/// Returns the retained marker index bytes to charge against a shared budget.
	#[must_use]
	pub fn metadata_bytes(&self) -> usize {
		self.roots.capacity() * size_of::<&str>()
	}

	/// Checks manifest membership and the original host path and file contents.
	/// Symlinks remain inert. The bounded Mach-O probe leaves non-Mach-O files alone.
	///
	/// # Errors
	/// Returns an error when a captured host path or bounded file read fails.
	pub fn protection(&self, entry: &Entry) -> Result<Option<Protection>, Error> {
		let source = entry.source.as_ref();
		let app_path = entry
			.destination
			.components()
			.any(|part| has_app_suffix(part.as_bytes()))
			|| source.is_some_and(|source| {
				source
					.path()
					.components()
					.any(|part| has_app_suffix(part.as_os_str().as_encoded_bytes()))
			});
		if app_path || inside_signed_bundle(entry.destination.as_str(), &self.roots) {
			return Ok(Some(Protection::Bundle));
		}
		if let Some(source) = source
			&& source.has_signed_bundle_marker()?
		{
			return Ok(Some(Protection::Bundle));
		}
		if let Some(source) = source
			&& matches!(entry.kind, EntryKind::File)
		{
			let file = CapturedFile::new(source.open_regular()?)?;
			let signed =
				dmgez_macho::has_or_may_have_signature_at(file.len(), |offset, output| {
					file.read_exact_at(offset, output)
				})?;
			source.validate()?;
			if signed {
				return Ok(Some(Protection::Executable));
			}
		}
		Ok(None)
	}
}
