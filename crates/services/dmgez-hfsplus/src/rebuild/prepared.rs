use std::{collections::TryReserveError, path::Path, rc::Rc};

use dmgez_builder::manifest::{self, CopyInput, Manifest};
use dmgez_config::{
	MetadataPolicy, VirtualPath,
	project_edit::{ContentDraft, ProjectEdit},
};
use hfsplus_builder::volume::EntryMetadata;
use thiserror::Error;

use crate::hfsplus::{HostCapture, HostCopyMetadataLoss, HostEntry, HostKind, capture_host_copies};

use super::{
	Attributes, File, Kind, LinkIdentity, Node, Payload,
	capture::{MAX_CAPTURE_BYTES, MAX_DEPTH, MAX_ENTRIES},
};

/// Failure to discover or retain the host files for one edited image.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum PrepareError {
	#[error("could not discover host input for the image edit")]
	Discovery(#[from] manifest::Error),
	#[error("could not retain host input metadata for the image edit")]
	Metadata(#[from] crate::Error),
	#[error("admitted HFS+ attribute name is not UTF-8")]
	AttributeName(#[from] std::string::FromUtf8Error),
	#[error("host input metadata exceeds the edit capture budget")]
	Budget,
	#[error("could not reserve host edit input memory")]
	Reserve(#[from] TryReserveError),
}

/// One complete edit batch and its bounded, identity-checked host inputs.
pub struct PreparedEdit {
	edits: Vec<ProjectEdit>,
	manifest: Option<Manifest>,
	copies: Vec<PreparedCopy>,
	losses: Vec<HostCopyMetadataLoss>,
	capture_budget: usize,
}

impl PreparedEdit {
	/// Captures `add_copy` inputs relative to the absolute edit batch directory.
	/// The output identifies the directory where the caller will stage its file.
	/// Capture inputs before creating that stage. The host policy applies only to
	/// the added files. Existing image objects still use strict preservation.
	/// # Errors
	/// Rejects invalid or unsafe sources, reuse of the output as input, exhausted
	/// limits, or a source that fails the requested metadata policy.
	pub fn new(
		edits: Vec<ProjectEdit>,
		changes_dir: &Path,
		output: &Path,
		policy: MetadataPolicy,
	) -> Result<Self, PrepareError> {
		let mut prepared = Self {
			edits,
			manifest: None,
			copies: Vec::new(),
			losses: Vec::new(),
			capture_budget: MAX_CAPTURE_BYTES,
		};
		let mut inputs = Vec::new();
		for edit in &prepared.edits {
			if let ProjectEdit::Add(ContentDraft::Copy {
				source,
				destination,
			}) = edit
			{
				inputs.try_reserve(1)?;
				inputs.push(CopyInput {
					source,
					destination: destination.as_ref(),
				});
			}
		}
		if !inputs.is_empty() {
			let limits = manifest::Limits {
				max_entries: MAX_ENTRIES as u64,
				max_visited: MAX_ENTRIES as u64,
				max_depth: MAX_DEPTH,
				max_metadata_bytes: MAX_CAPTURE_BYTES as u64,
			};
			let manifest = manifest::discover_copies(&inputs, changes_dir, output, limits)?;
			let HostCapture {
				entries,
				losses,
				accounted_bytes,
			} = capture_host_copies(&manifest, policy, MAX_CAPTURE_BYTES)?;
			prepared.capture_budget = MAX_CAPTURE_BYTES
				.checked_sub(accounted_bytes)
				.ok_or(PrepareError::Budget)?;
			prepared.copies.try_reserve_exact(entries.len())?;
			for entry in entries {
				let copy = PreparedCopy::from_entry(entry)?;
				// Sealing temporarily holds the planned host entries alongside the
				// captured source manifest and the original image catalog.
				prepared.capture_budget = prepared
					.capture_budget
					.checked_sub(copy.overlay_bytes())
					.ok_or(PrepareError::Budget)?;
				prepared.copies.push(copy);
			}
			prepared.manifest = Some(manifest);
			prepared.losses = losses;
		}
		Ok(prepared)
	}

	/// Number of declarations in the original edit batch.
	#[must_use]
	pub fn edits_count(&self) -> usize {
		self.edits.len()
	}

	/// Explicit metadata losses admitted under report policy for added content.
	#[must_use]
	pub fn metadata_losses(&self) -> &[HostCopyMetadataLoss] {
		&self.losses
	}

	/// Revalidate every captured host entry, including directories and symlinks.
	/// # Errors
	/// Returns an error if a source changed or can no longer be inspected.
	pub fn validate_sources(&self) -> Result<(), manifest::Error> {
		if let Some(manifest) = &self.manifest {
			manifest.validate_sources()?;
		}
		Ok(())
	}

	pub(super) fn edits(&self) -> &[ProjectEdit] {
		&self.edits
	}

	pub(super) const fn capture_budget(&self) -> usize {
		self.capture_budget
	}

	pub(super) fn nodes<S>(&self) -> Result<Vec<(VirtualPath, Node<'_, S>)>, TryReserveError> {
		let mut nodes = Vec::new();
		for entry in &self.copies {
			nodes.try_reserve(1 + entry.aliases.len())?;
			match &entry.kind {
				HostKind::Directory => {
					nodes.push((entry.path.clone(), entry.node(Kind::Directory)))
				}
				HostKind::Symlink(target) => nodes.push((
					entry.path.clone(),
					entry.node(Kind::Symlink(target.clone())),
				)),
				HostKind::File {
					source,
					resource,
					identity,
				} => {
					let file = Rc::new(File {
						data: Payload::HostFile(source),
						resource: resource.as_deref().map(Payload::HostBytes),
					});
					let inode = identity.map(LinkIdentity::Host);
					for path in std::iter::once(&entry.path).chain(&entry.aliases) {
						nodes.push((
							path.clone(),
							entry.node(Kind::File {
								file: Rc::clone(&file),
								inode,
							}),
						));
					}
				}
			}
		}
		Ok(nodes)
	}
}

struct PreparedCopy {
	path: VirtualPath,
	aliases: Vec<VirtualPath>,
	metadata: EntryMetadata,
	kind: HostKind,
	attributes: Attributes,
}

impl PreparedCopy {
	fn overlay_bytes(&self) -> usize {
		let path_bytes = std::iter::once(&self.path)
			.chain(&self.aliases)
			.map(|path| size_of::<(VirtualPath, Node<'_, ()>)>() + path.as_str().len())
			.sum::<usize>();
		let extra = match &self.kind {
			HostKind::Directory => 0,
			HostKind::File { .. } => size_of::<File<'_, ()>>(),
			HostKind::Symlink(target) => target.len(),
		};
		// The copy count and retained path/payload bytes were already bounded
		// by discovery and capture, so these sums cannot overflow.
		path_bytes + extra
	}

	fn from_entry(entry: HostEntry) -> Result<Self, PrepareError> {
		let attributes = entry
			.attributes
			.into_iter()
			.map(|attribute| Ok((String::from_utf8(attribute.name)?, attribute.value)))
			.collect::<Result<Attributes, std::string::FromUtf8Error>>()?;
		Ok(Self {
			path: entry.path,
			aliases: entry.aliases,
			metadata: entry.metadata,
			kind: entry.kind,
			attributes,
		})
	}

	fn node<'volume, S>(&self, kind: Kind<'volume, S>) -> Node<'volume, S> {
		Node {
			metadata: self.metadata,
			kind,
			attributes: Rc::clone(&self.attributes),
		}
	}
}
