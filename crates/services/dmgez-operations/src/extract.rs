//! Bounded extraction plans and publication for explicitly selected file content.

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub mod host;

mod names;
mod plan;
mod report;

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod execute;

use std::{
	collections::{HashMap, HashSet, TryReserveError},
	hash::Hash,
	num::NonZeroU16,
};

use dmgez_vfs::volume::{NodeHandle, NodeKind, ViewError, VolumeBackend, VolumeView};
use thiserror::Error;

pub use names::{HostName, NameError, NameProfile};
pub use plan::{ExtractionLimits, ExtractionPlan, PlanError, PlannedFork, Selection, prepare};
pub use report::{Omission, PlanReport, Progress, SelectionReport};

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub use execute::{ExecutionError, ExecutionFailure, execute};

const DEFAULT_PAGE_ENTRIES: NonZeroU16 = NonZeroU16::new(64).unwrap();
const DEFAULT_MAX_ENTRIES: usize = 16_384;
const DEFAULT_MAX_PAGES: usize = 32_768;
const DEFAULT_MAX_DEPTH: usize = 128;
const DEFAULT_COMPONENT_BYTES: usize = 255;
const DEFAULT_RELATIVE_PATH_BYTES: usize = 4095;
const DEFAULT_TOTAL_NAME_BYTES: usize = 2 * 1024 * 1024;

/// Bounds the traversal and the retained destination names.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NameLimits {
	pub page_entries: NonZeroU16,
	pub max_entries: usize,
	pub max_pages: usize,
	pub max_depth: usize,
	pub max_component_bytes: usize,
	pub max_relative_path_bytes: usize,
	pub max_total_name_bytes: usize,
}

impl Default for NameLimits {
	fn default() -> Self {
		Self {
			page_entries: DEFAULT_PAGE_ENTRIES,
			max_entries: DEFAULT_MAX_ENTRIES,
			max_pages: DEFAULT_MAX_PAGES,
			max_depth: DEFAULT_MAX_DEPTH,
			max_component_bytes: DEFAULT_COMPONENT_BYTES,
			max_relative_path_bytes: DEFAULT_RELATIVE_PATH_BYTES,
			max_total_name_bytes: DEFAULT_TOTAL_NAME_BYTES,
		}
	}
}

/// Position of an entry inside this plan. The destination root has no entry.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct EntryId(usize);

impl EntryId {
	#[must_use]
	pub const fn index(self) -> usize {
		self.0
	}
}

/// One destination component with its original, session-bound source handle.
#[derive(Debug)]
#[non_exhaustive]
pub struct PlannedEntry<Id> {
	pub parent: Option<EntryId>,
	pub name: HostName,
	pub kind: NodeKind,
	pub source: NodeHandle<Id>,
}

/// A read-only plan in parent-before-child order, scoped to one source view.
#[derive(Debug)]
pub struct NamePlan<Id> {
	profile: NameProfile,
	entries: Vec<PlannedEntry<Id>>,
}

impl<Id> NamePlan<Id> {
	#[must_use]
	pub const fn profile(&self) -> NameProfile {
		self.profile
	}

	#[must_use]
	pub fn entries(&self) -> &[PlannedEntry<Id>] {
		&self.entries
	}
}

/// Aggregate limit that prevented the plan from being completed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum NameBudget {
	Entries,
	Pages,
	Depth,
	RelativePathBytes,
	TotalNameBytes,
}

/// Failed to plan every entry under the selected source directory.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum NamePlanError<E: std::error::Error + 'static> {
	#[error("cannot read the selected source directory")]
	Source(#[from] ViewError<E>),
	#[error("cannot preserve destination entry {entry:?}")]
	Name {
		entry: EntryId,
		#[source]
		reason: NameError,
	},
	#[error("destination names {first:?} and {second:?} collide in one directory")]
	Collision { first: HostName, second: HostName },
	#[error("source directory is reachable through multiple entries or a cycle")]
	RepeatedDirectory,
	#[error("name plan exceeds the {0:?} budget")]
	Limit(NameBudget),
	#[error("cannot reserve extraction name planning memory")]
	Reserve(#[from] TryReserveError),
	#[error("extraction name preflight cancelled")]
	Cancelled,
}

struct Pending<Id> {
	node: NodeHandle<Id>,
	entry: Option<EntryId>,
	depth: usize,
	path_bytes: usize,
}

/// Plans the names of the selected directory's contents without opening host paths.
///
/// The caller supplies the destination's qualified name profile. A successful
/// plan checks confined components, per-directory name collisions, and bounded
/// source topology. Link objects remain in the plan for later policy checks.
/// Host identities, existing entries, link targets, metadata, and fork bytes
/// must be checked by the subsequent extraction stages.
///
/// # Errors
/// Rejects foreign or unreadable source directories, unrepresentable or
/// colliding names, repeated directories, exceeded limits, and cancellation.
pub fn preflight_names<B: VolumeBackend>(
	view: &mut VolumeView<B>,
	directory: &NodeHandle<B::Id>,
	profile: NameProfile,
	limits: NameLimits,
	mut cancelled: impl FnMut() -> bool,
) -> Result<NamePlan<B::Id>, NamePlanError<B::Error>>
where
	B::Id: Eq + Hash,
{
	let mut entries: Vec<PlannedEntry<B::Id>> = Vec::new();
	let mut seen = HashSet::new();
	seen.try_reserve(1)?;
	seen.insert(directory.clone());
	let mut pending = Vec::new();
	pending.try_reserve(1)?;
	pending.push(Pending {
		node: directory.clone(),
		entry: None,
		depth: 0,
		path_bytes: 0,
	});
	let mut pages = 0;
	let mut retained_name_bytes = 0;
	while let Some(parent) = pending.pop() {
		if cancelled() {
			return Err(NamePlanError::Cancelled);
		}
		let mut names = HashMap::<String, EntryId>::new();
		let mut scan = view.read_dir(&parent.node)?;
		loop {
			if cancelled() {
				return Err(NamePlanError::Cancelled);
			}
			pages = charge(pages, 1, limits.max_pages, NameBudget::Pages)?;
			let page = scan.next_page(limits.page_entries)?;
			charge(
				entries.len(),
				page.entries.len(),
				limits.max_entries,
				NameBudget::Entries,
			)?;
			entries.try_reserve(page.entries.len())?;
			names.try_reserve(page.entries.len())?;
			for child in page.entries {
				if cancelled() {
					return Err(NamePlanError::Cancelled);
				}
				let depth = charge(parent.depth, 1, limits.max_depth, NameBudget::Depth)?;
				let id = EntryId(entries.len());
				let name = HostName::admit(child.name, profile, limits.max_component_bytes)
					.map_err(|reason| NamePlanError::Name { entry: id, reason })?;
				let name_bytes = name.as_str().len();
				retained_name_bytes = charge(
					retained_name_bytes,
					name_bytes,
					limits.max_total_name_bytes,
					NameBudget::TotalNameBytes,
				)?;
				let path_bytes = names::charge_child_path(
					parent.path_bytes,
					parent.entry.is_some(),
					name_bytes,
					limits.max_relative_path_bytes,
				)?;
				if let Some(first) = names.insert(name.comparison_key(profile), id) {
					return Err(NamePlanError::Collision {
						first: entries[first.index()].name.clone(),
						second: name,
					});
				}
				if child.kind == NodeKind::Directory {
					seen.try_reserve(1)?;
					if !seen.insert(child.node.clone()) {
						return Err(NamePlanError::RepeatedDirectory);
					}
					pending.try_reserve(1)?;
					pending.push(Pending {
						node: child.node.clone(),
						entry: Some(id),
						depth,
						path_bytes,
					});
				}
				entries.push(PlannedEntry {
					parent: parent.entry,
					name,
					kind: child.kind,
					source: child.node,
				});
			}
			if page.complete {
				break;
			}
		}
	}
	Ok(NamePlan { profile, entries })
}

fn charge<E: std::error::Error + 'static>(
	used: usize,
	amount: usize,
	limit: usize,
	budget: NameBudget,
) -> Result<usize, NamePlanError<E>> {
	used.checked_add(amount)
		.filter(|&next| next <= limit)
		.ok_or(NamePlanError::Limit(budget))
}
