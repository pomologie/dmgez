use std::{collections::TryReserveError, error::Error as StdError, num::NonZeroU32};

use dmgez_vfs::volume::{
	FINDER_INFO_BYTES, FinderInfo, ForkKind, NodeHandle, NodeKind, Quarantine, QuarantineScope,
	RawName, ViewError, VolumeBackend, VolumeView,
};
use thiserror::Error;

use crate::extract::{EntryId, HostName};

use super::PlannedApplication;

mod link;

const GIBIBYTE: u64 = 1 << 30;
const MEBIBYTE: u64 = 1 << 20;
const MAX_ATTRIBUTES_PER_ENTRY: NonZeroU32 = NonZeroU32::new(1_024).unwrap();
const MAX_ATTRIBUTE_NAME_BYTES: usize = 127;
const ORDINARY_MODE: u16 = 0o777;
const SPECIAL_MODE: u16 = 0o7000;
const OWNER_DIRECTORY_ACCESS: u16 = 0o500;
pub(super) const FINDER_ATTRIBUTE: &str = "com.apple.FinderInfo";
pub(super) const RESOURCE_ATTRIBUTE: &str = "com.apple.ResourceFork";
const COMPRESSION_ATTRIBUTE: &str = "com.apple.decmpfs";

/// Bounds for the inventory of one application copy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CopyLimits {
	/// Sum of all data forks, resource forks, attribute values, and link targets.
	pub max_payload_bytes: u64,
	/// Largest individually staged attribute value, including a resource fork.
	pub max_attribute_bytes: u64,
	/// Maximum number of retained attribute names and FinderInfo values.
	pub max_attributes: usize,
}

impl Default for CopyLimits {
	fn default() -> Self {
		Self {
			max_payload_bytes: 4 * GIBIBYTE,
			max_attribute_bytes: 16 * MEBIBYTE,
			max_attributes: 32_768,
		}
	}
}

/// Bounded inventory for staging one application from a held immutable view.
#[derive(Debug)]
pub struct CopyPlan<Id> {
	application: PlannedApplication<Id>,
	root: CopyEntry,
	entries: Vec<CopyEntry>,
	summary: CopySummary,
}

impl<Id> CopyPlan<Id> {
	#[must_use]
	pub const fn application(&self) -> &PlannedApplication<Id> {
		&self.application
	}

	#[must_use]
	pub const fn summary(&self) -> CopySummary {
		self.summary
	}

	/// Copy metadata for the application's root directory.
	#[must_use]
	pub const fn root(&self) -> &CopyEntry {
		&self.root
	}

	/// Copy metadata in the same order as the application's name plan.
	#[must_use]
	pub fn entries(&self) -> &[CopyEntry] {
		&self.entries
	}
}

/// Exact mode and admitted copy streams of a source object.
#[derive(Debug)]
pub struct CopyEntry {
	pub mode: u16,
	pub content: CopyContent,
	pub finder_info: Option<[u8; FINDER_INFO_BYTES]>,
	pub attributes: Vec<CopyAttribute>,
}

/// Source payload to reproduce. Relative link targets retain their raw bytes.
#[derive(Debug)]
#[non_exhaustive]
pub enum CopyContent {
	Directory,
	File {
		data_bytes: u64,
		resource_bytes: Option<u64>,
		/// Some filesystems expose the same fork as a named attribute. The
		/// executor must confirm the values agree before projecting one copy.
		resource_alias: Option<RawName>,
	},
	Link(Vec<u8>),
}

/// Exact stored lookup name, host spelling, and admitted value length.
#[derive(Debug)]
pub struct CopyAttribute {
	pub source_name: RawName,
	pub host_name: String,
	pub bytes: u64,
}

/// Copy inventory size before any destination mutation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CopySummary {
	pub payload_bytes: u64,
	pub attributes: usize,
}

/// A bundle could not be admitted for copy with its selected metadata.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum CopyPlanError<E: StdError + 'static> {
	#[error("could not inspect the selected application's copy metadata")]
	Source(#[from] ViewError<E>),
	#[error("application copy planning was cancelled")]
	Cancelled,
	#[error("application entry {0:?} has marked or unverifiable quarantine state")]
	Quarantine(Option<EntryId>),
	#[error("application entry {0:?} has an unsupported object kind")]
	Kind(Option<EntryId>),
	#[error("application entry {0:?} has unsupported permission bits")]
	Mode(Option<EntryId>),
	#[error("application entry {0:?} has a shared hard-link group")]
	HardLink(Option<EntryId>),
	#[error("application entry {0:?} has an unsafe or unsupported link target")]
	Link(Option<EntryId>),
	#[error("application entry {0:?} has no data fork")]
	Data(Option<EntryId>),
	#[error("application entry {0:?} has unavailable or unrepresentable attributes")]
	Attributes(Option<EntryId>),
	#[error("application entry {0:?} has an attribute value beyond the copy budget")]
	AttributeLimit(Option<EntryId>),
	#[error("application copy inventory exceeds its attribute count budget")]
	AttributeCount,
	#[error("application copy exceeds its payload byte budget")]
	PayloadLimit,
	#[error("could not reserve application copy inventory memory")]
	Reserve(#[from] TryReserveError),
}

/// Qualifies the data and metadata streams for a selected application.
///
/// Plans against the same immutable view as the layout preflight. Marked
/// quarantine, special mode bits, shared hard links, unsupported attribute
/// representations, and unconfined links are refused before staging. Name
/// planning already bounds topology. Ordinary fork payloads remain in the view.
///
/// # Errors
/// Returns a typed admission, source, budget, or cancellation failure.
pub fn prepare_copy<B: VolumeBackend>(
	view: &mut VolumeView<B>,
	application: PlannedApplication<B::Id>,
	limits: CopyLimits,
	mut cancelled: impl FnMut() -> bool,
) -> Result<CopyPlan<B::Id>, CopyPlanError<B::Error>> {
	let count = application.names().entries().len();
	let mut entries = Vec::new();
	let mut depths = Vec::new();
	entries.try_reserve_exact(count)?;
	depths.try_reserve_exact(count)?;
	let mut budget = Budget {
		limits,
		summary: CopySummary::default(),
	};
	let profile = application.names().profile();
	let mut scan = Scan {
		view,
		budget: &mut budget,
		profile,
		cancelled: &mut cancelled,
	};
	let root = scan.entry(application.source(), None, 0)?;
	for (index, entry) in application.names().entries().iter().enumerate() {
		let depth = entry.parent.map_or(0, |parent| depths[parent.index()]);
		entries.push(scan.entry(&entry.source, Some(EntryId::from_index(index)), depth)?);
		depths.push(depth + 1);
	}
	Ok(CopyPlan {
		application,
		root,
		entries,
		summary: budget.summary,
	})
}

struct Budget {
	limits: CopyLimits,
	summary: CopySummary,
}

impl Budget {
	fn charge<E: StdError + 'static>(&mut self, bytes: u64) -> Result<(), CopyPlanError<E>> {
		self.summary.payload_bytes = self
			.summary
			.payload_bytes
			.checked_add(bytes)
			.filter(|&size| size <= self.limits.max_payload_bytes)
			.ok_or(CopyPlanError::PayloadLimit)?;
		Ok(())
	}

	fn attribute<E: StdError + 'static>(
		&mut self,
		bytes: u64,
		entry: Option<EntryId>,
	) -> Result<(), CopyPlanError<E>> {
		if bytes > self.limits.max_attribute_bytes {
			return Err(CopyPlanError::AttributeLimit(entry));
		}
		self.summary.attributes = self
			.summary
			.attributes
			.checked_add(1)
			.filter(|&count| count <= self.limits.max_attributes)
			.ok_or(CopyPlanError::AttributeCount)?;
		self.charge(bytes)
	}
}

struct Scan<'view, 'limits, B> {
	view: &'view mut VolumeView<B>,
	budget: &'limits mut Budget,
	profile: crate::extract::NameProfile,
	cancelled: &'limits mut dyn FnMut() -> bool,
}

impl<B: VolumeBackend> Scan<'_, '_, B> {
	fn entry(
		&mut self,
		node: &NodeHandle<B::Id>,
		id: Option<EntryId>,
		parent_depth: usize,
	) -> Result<CopyEntry, CopyPlanError<B::Error>> {
		self.check_cancelled()?;
		let scope = if id.is_none() {
			QuarantineScope::Ancestors
		} else {
			QuarantineScope::Object
		};
		if self.view.quarantine(node, scope)? != Quarantine::Clear {
			return Err(CopyPlanError::Quarantine(id));
		}
		let metadata = self.view.stat(node)?;
		if metadata
			.hard_link
			.is_some_and(|group| group.count.get() > 1)
		{
			return Err(CopyPlanError::HardLink(id));
		}
		let mode = metadata.mode & ORDINARY_MODE;
		if metadata.mode & SPECIAL_MODE != 0
			|| metadata.kind == NodeKind::Directory
				&& mode & OWNER_DIRECTORY_ACCESS != OWNER_DIRECTORY_ACCESS
		{
			return Err(CopyPlanError::Mode(id));
		}
		let names = self
			.view
			.list_xattrs(node, MAX_ATTRIBUTES_PER_ENTRY)?
			.ok_or(CopyPlanError::Attributes(id))?;
		let finder_info = match self.view.finder_info(node)? {
			FinderInfo::Present(value) => {
				self.budget.attribute(value.len() as u64, id)?;
				Some(value)
			}
			FinderInfo::Absent => None,
			FinderInfo::Unavailable => return Err(CopyPlanError::Attributes(id)),
		};
		let mut content = match metadata.kind {
			NodeKind::Directory => CopyContent::Directory,
			NodeKind::RegularFile => self.file(node, id)?,
			NodeKind::Symlink => {
				let target = self.view.read_link(node)?;
				if !link::confined(&target, parent_depth, self.profile) {
					return Err(CopyPlanError::Link(id));
				}
				self.budget.charge(target.len() as u64)?;
				CopyContent::Link(target)
			}
			_ => return Err(CopyPlanError::Kind(id)),
		};
		let attributes = self.attributes(node, id, names, &mut content, finder_info.as_ref())?;
		if let CopyContent::File {
			resource_bytes,
			resource_alias,
			..
		} = &mut content
		{
			if *resource_bytes == Some(0) && resource_alias.is_none() {
				// Readers of filesystems with an implicit resource descriptor
				// can report an empty resource even when no attribute is stored.
				*resource_bytes = None;
			}
			if let Some(bytes) = *resource_bytes {
				self.budget.attribute(bytes, id)?;
			}
		}
		if matches!(content, CopyContent::Link(_))
			&& (!attributes.is_empty() || finder_info.is_some())
		{
			return Err(CopyPlanError::Attributes(id));
		}
		Ok(CopyEntry {
			mode,
			content,
			finder_info,
			attributes,
		})
	}

	fn file(
		&mut self,
		node: &NodeHandle<B::Id>,
		id: Option<EntryId>,
	) -> Result<CopyContent, CopyPlanError<B::Error>> {
		let data_bytes = self
			.view
			.with_fork(node, ForkKind::Data, |fork| fork.len())?
			.ok_or(CopyPlanError::Data(id))?;
		if i64::try_from(data_bytes).is_err() {
			return Err(CopyPlanError::PayloadLimit);
		}
		self.budget.charge(data_bytes)?;
		let resource_bytes = self
			.view
			.with_fork(node, ForkKind::Resource, |fork| fork.len())?;
		Ok(CopyContent::File {
			data_bytes,
			resource_bytes,
			resource_alias: None,
		})
	}

	fn attributes(
		&mut self,
		node: &NodeHandle<B::Id>,
		id: Option<EntryId>,
		names: Vec<RawName>,
		content: &mut CopyContent,
		finder_info: Option<&[u8; FINDER_INFO_BYTES]>,
	) -> Result<Vec<CopyAttribute>, CopyPlanError<B::Error>> {
		let mut attributes = Vec::new();
		attributes.try_reserve_exact(names.len())?;
		let mut finder_alias = false;
		for source_name in names {
			self.check_cancelled()?;
			let host_name = attribute_name(&source_name).ok_or(CopyPlanError::Attributes(id))?;
			match host_name.as_str() {
				COMPRESSION_ATTRIBUTE => return Err(CopyPlanError::Attributes(id)),
				FINDER_ATTRIBUTE => {
					if std::mem::replace(&mut finder_alias, true) {
						return Err(CopyPlanError::Attributes(id));
					}
					let expected = finder_info.ok_or(CopyPlanError::Attributes(id))?;
					let matches = self.view.with_xattr(
						node,
						source_name.as_lookup(),
						|fork| -> Result<_, ViewError<_>> {
							let mut actual = [0; FINDER_INFO_BYTES];
							if fork.len() != actual.len() as u64 {
								return Ok(false);
							}
							fork.read_exact_at(0, &mut actual)?;
							Ok(actual == *expected)
						},
					)?;
					if !matches.transpose()?.unwrap_or(false) {
						return Err(CopyPlanError::Attributes(id));
					}
				}
				RESOURCE_ATTRIBUTE => {
					let CopyContent::File {
						resource_bytes,
						resource_alias,
						..
					} = content
					else {
						return Err(CopyPlanError::Attributes(id));
					};
					if resource_alias.is_some() {
						return Err(CopyPlanError::Attributes(id));
					}
					let bytes = self.attribute_size(node, id, &source_name)?;
					if *resource_bytes != Some(bytes) {
						return Err(CopyPlanError::Attributes(id));
					}
					*resource_alias = Some(source_name);
				}
				_ => {
					let bytes = self.attribute_size(node, id, &source_name)?;
					self.budget.attribute(bytes, id)?;
					attributes.push(CopyAttribute {
						source_name,
						host_name,
						bytes,
					});
				}
			}
		}
		attributes.sort_unstable_by(|a, b| a.host_name.cmp(&b.host_name));
		if attributes
			.windows(2)
			.any(|pair| pair[0].host_name == pair[1].host_name)
		{
			return Err(CopyPlanError::Attributes(id));
		}
		Ok(attributes)
	}

	fn attribute_size(
		&mut self,
		node: &NodeHandle<B::Id>,
		id: Option<EntryId>,
		name: &RawName,
	) -> Result<u64, CopyPlanError<B::Error>> {
		self.view
			.with_xattr(node, name.as_lookup(), |fork| fork.len())?
			.ok_or(CopyPlanError::Attributes(id))
	}

	fn check_cancelled(&mut self) -> Result<(), CopyPlanError<B::Error>> {
		if (self.cancelled)() {
			return Err(CopyPlanError::Cancelled);
		}
		Ok(())
	}
}

fn attribute_name(name: &RawName) -> Option<String> {
	let value = match name {
		RawName::Utf8(name) if name.len() <= MAX_ATTRIBUTE_NAME_BYTES => name.clone(),
		RawName::Utf16(units) if units.len() <= MAX_ATTRIBUTE_NAME_BYTES => {
			String::from_utf16(units).ok()?
		}
		_ => return None,
	};
	HostName::new(
		&value,
		crate::extract::NameProfile::PosixUtf8,
		MAX_ATTRIBUTE_NAME_BYTES,
	)
	.ok()?;
	Some(value)
}
