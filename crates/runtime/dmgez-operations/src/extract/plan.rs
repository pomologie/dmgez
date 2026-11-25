use std::{collections::TryReserveError, error::Error as StdError, hash::Hash, ops::Range};

use dmgez_vfs::volume::{
	ForkKind, Metadata, NodeHandle, NodeKind, Quarantine, QuarantineScope, StorageId, ViewError,
	VolumeBackend, VolumeView,
};
use thiserror::Error;

use super::{
	DEFAULT_MAX_DEPTH, EntryId, HostName, NameLimits, NamePlanError, NameProfile, PlanReport,
	SelectionReport, preflight_names,
};

const DEFAULT_TOTAL_BYTES: u64 = 4 * (1 << 30);

/// Requested content projection.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[non_exhaustive]
pub enum Selection {
	/// Materialize one explicitly selected fork as an ordinary file.
	Fork(ForkKind),
	/// Materialize regular data forks and directories, omitting other metadata.
	DataTree,
}

/// Limits for name planning, fork opening, and the total bytes to publish.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct ExtractionLimits {
	pub profile: NameProfile,
	pub names: NameLimits,
	pub max_fork_bytes: u64,
	pub max_total_bytes: u64,
}

impl Default for ExtractionLimits {
	fn default() -> Self {
		Self {
			profile: NameProfile::PortableAscii,
			names: NameLimits {
				// Leave room for the new destination root in the host's directory chain.
				max_depth: DEFAULT_MAX_DEPTH - 1,
				..NameLimits::default()
			},
			max_fork_bytes: DEFAULT_TOTAL_BYTES,
			max_total_bytes: DEFAULT_TOTAL_BYTES,
		}
	}
}

/// An admitted plan tied to one immutable volume view.
#[derive(Debug)]
pub struct ExtractionPlan<Id> {
	pub(super) source: NodeHandle<Id>,
	pub(super) contents: Contents<Id>,
	pub(super) limits: ExtractionLimits,
	report: PlanReport,
}

impl<Id> ExtractionPlan<Id> {
	#[must_use]
	pub const fn report(&self) -> &PlanReport {
		&self.report
	}

	#[must_use]
	pub const fn limits(&self) -> ExtractionLimits {
		self.limits
	}

	/// Visits prepared forks in the plan's enumeration order without reading data.
	pub fn forks(&self) -> impl Iterator<Item = PlannedFork<'_, Id>> {
		let (single, tree) = match &self.contents {
			Contents::File(file) => (Some(file.describe(&self.source, None)), &[][..]),
			Contents::Tree(tree) => (None, tree.entries.as_slice()),
		};
		single
			.into_iter()
			.chain(tree.iter().filter_map(|entry| match &entry.item {
				TreeItem::File(file) => Some(file.describe(&entry.source, Some(&entry.name))),
				TreeItem::Directory(_) => None,
			}))
	}
}

/// Read-only details of one selected fork in a prepared projection.
#[derive(Debug)]
#[non_exhaustive]
pub struct PlannedFork<'plan, Id> {
	pub source: &'plan NodeHandle<Id>,
	/// A single-fork projection takes its output basename from the caller.
	pub name: Option<&'plan HostName>,
	pub kind: ForkKind,
	pub bytes: u64,
}

#[derive(Debug)]
pub(super) enum Contents<Id> {
	File(File),
	Tree(Tree<Id>),
}

#[derive(Debug)]
pub(super) struct File {
	pub(super) fork: ForkKind,
	pub(super) bytes: u64,
}

impl File {
	fn describe<'plan, Id>(
		&self,
		source: &'plan NodeHandle<Id>,
		name: Option<&'plan HostName>,
	) -> PlannedFork<'plan, Id> {
		PlannedFork {
			source,
			name,
			kind: self.fork,
			bytes: self.bytes,
		}
	}
}

#[derive(Debug)]
pub(super) struct Tree<Id> {
	pub(super) children: Range<usize>,
	pub(super) entries: Vec<TreeEntry<Id>>,
}

#[derive(Debug)]
pub(super) struct TreeEntry<Id> {
	pub(super) name: HostName,
	pub(super) source: NodeHandle<Id>,
	pub(super) item: TreeItem,
}

#[derive(Debug)]
pub(super) enum TreeItem {
	Directory(Range<usize>),
	File(File),
}

/// Extraction could not be planned without changing the destination.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum PlanError<E: StdError + 'static> {
	#[error(transparent)]
	Names(#[from] NamePlanError<E>),
	#[error("could not inspect the selected source metadata")]
	Metadata(#[source] ViewError<E>),
	#[error("could not check the source quarantine state")]
	Quarantine(#[source] ViewError<E>),
	#[error("the selected source or a tree entry is quarantined")]
	Quarantined { entry: Option<EntryId> },
	#[error("the selected source or a tree entry has unavailable quarantine information")]
	QuarantineUnavailable { entry: Option<EntryId> },
	#[error("fork extraction requires a regular file, found {0:?}")]
	ExpectedFile(NodeKind),
	#[error("tree extraction requires a directory, found {0:?}")]
	ExpectedDirectory(NodeKind),
	#[error("tree entry {entry:?} has unsupported kind {kind:?}")]
	EntryKind { entry: EntryId, kind: NodeKind },
	#[error("the requested fork kind is unsupported")]
	UnsupportedFork,
	#[error("the selected {0:?} fork is absent")]
	AbsentFork(ForkKind),
	#[error("could not open the selected source fork")]
	Fork(#[source] ViewError<E>),
	#[error("the selected fork exceeds the fork byte budget")]
	ForkBytes,
	#[error("extraction exceeds the total byte budget")]
	TotalBytes,
	#[error("could not reserve extraction planning memory")]
	Reserve(#[from] TryReserveError),
	#[error("extraction planning was cancelled")]
	Cancelled,
}

/// Plans a selected file fork or data-only directory tree without writing anything.
///
/// Each selected fork must open and fit the limits. Planning does not read every
/// payload byte. Symbolic links and special objects cannot enter a data tree.
///
/// # Errors
/// Rejects unsupported objects and forks, unrepresentable names, exceeded limits,
/// unreadable metadata or fork descriptors, and cancellation.
pub fn prepare<B: VolumeBackend>(
	view: &mut VolumeView<B>,
	node: &NodeHandle<B::Id>,
	selection: Selection,
	limits: ExtractionLimits,
	mut cancelled: impl FnMut() -> bool,
) -> Result<ExtractionPlan<B::Id>, PlanError<B::Error>>
where
	B::Id: Eq + Hash,
{
	check_cancel(&mut cancelled)?;
	let metadata = view.stat(node).map_err(PlanError::Metadata)?;
	check_quarantine(view, node, QuarantineScope::Ancestors, None)?;
	let (contents, report) = match selection {
		Selection::Fork(fork) => {
			if metadata.kind != NodeKind::RegularFile {
				return Err(PlanError::ExpectedFile(metadata.kind));
			}
			let kind = match fork {
				ForkKind::Data => SelectionReport::DataFork,
				ForkKind::Resource => SelectionReport::ResourceFork,
				_ => return Err(PlanError::UnsupportedFork),
			};
			let mut report = PlanReport::new(kind);
			let file = plan_file(view, node, &metadata, fork, limits, &mut report)?;
			(Contents::File(file), report)
		}
		Selection::DataTree => {
			if metadata.kind != NodeKind::Directory {
				return Err(PlanError::ExpectedDirectory(metadata.kind));
			}
			let mut report = PlanReport::new(SelectionReport::DataTree);
			let tree = plan_tree(view, node, limits, &mut report, &mut cancelled)?;
			(Contents::Tree(tree), report)
		}
	};
	check_cancel(&mut cancelled)?;
	Ok(ExtractionPlan {
		source: node.clone(),
		contents,
		limits,
		report,
	})
}

fn plan_tree<B: VolumeBackend>(
	view: &mut VolumeView<B>,
	node: &NodeHandle<B::Id>,
	limits: ExtractionLimits,
	report: &mut PlanReport,
	cancelled: &mut impl FnMut() -> bool,
) -> Result<Tree<B::Id>, PlanError<B::Error>>
where
	B::Id: Eq + Hash,
{
	let names_limits = NameLimits {
		max_depth: limits.names.max_depth.min(DEFAULT_MAX_DEPTH - 1),
		..limits.names
	};
	let names = preflight_names(view, node, limits.profile, names_limits, &mut *cancelled)?;
	let mut tree = Tree {
		children: 0..0,
		entries: Vec::new(),
	};
	tree.entries.try_reserve_exact(names.entries.len())?;
	report.directories = 1;
	for (index, entry) in names.entries.into_iter().enumerate() {
		check_cancel(cancelled)?;
		check_quarantine(
			view,
			&entry.source,
			QuarantineScope::Object,
			Some(EntryId(index)),
		)?;
		let item = match entry.kind {
			NodeKind::Directory => {
				report.directories += 1;
				TreeItem::Directory(0..0)
			}
			NodeKind::RegularFile => {
				let metadata = view.stat(&entry.source).map_err(PlanError::Metadata)?;
				TreeItem::File(plan_file(
					view,
					&entry.source,
					&metadata,
					ForkKind::Data,
					limits,
					report,
				)?)
			}
			kind => {
				return Err(PlanError::EntryKind {
					entry: EntryId(index),
					kind,
				});
			}
		};
		let children = match entry.parent {
			None => &mut tree.children,
			Some(parent) => {
				let TreeItem::Directory(children) = &mut tree.entries[parent.index()].item else {
					unreachable!("name preflight admits only directories as parents");
				};
				children
			}
		};
		// Name preflight enumerates each parent completely before the next directory.
		*children = if children.start == children.end {
			index..index + 1
		} else {
			debug_assert_eq!(children.end, index);
			children.start..index + 1
		};
		tree.entries.push(TreeEntry {
			name: entry.name,
			source: entry.source,
			item,
		});
	}
	Ok(tree)
}

fn plan_file<B: VolumeBackend>(
	view: &mut VolumeView<B>,
	node: &NodeHandle<B::Id>,
	metadata: &Metadata<StorageId>,
	fork: ForkKind,
	limits: ExtractionLimits,
	report: &mut PlanReport,
) -> Result<File, PlanError<B::Error>> {
	let bytes = view
		.with_fork(node, fork, |opened| opened.len())
		.map_err(PlanError::Fork)?
		.ok_or(PlanError::AbsentFork(fork))?;
	if bytes > limits.max_fork_bytes || i64::try_from(bytes).is_err() {
		return Err(PlanError::ForkBytes);
	}
	report.bytes = report
		.bytes
		.checked_add(bytes)
		.filter(|sum| *sum <= limits.max_total_bytes)
		.ok_or(PlanError::TotalBytes)?;
	report.files += 1;
	report.hard_links_materialized += usize::from(metadata.hard_link.is_some());
	Ok(File { fork, bytes })
}

fn check_quarantine<B: VolumeBackend>(
	view: &mut VolumeView<B>,
	node: &NodeHandle<B::Id>,
	scope: QuarantineScope,
	entry: Option<EntryId>,
) -> Result<(), PlanError<B::Error>> {
	match view
		.quarantine(node, scope)
		.map_err(PlanError::Quarantine)?
	{
		Quarantine::Clear => Ok(()),
		Quarantine::Marked => Err(PlanError::Quarantined { entry }),
		_ => Err(PlanError::QuarantineUnavailable { entry }),
	}
}

fn check_cancel<E: StdError + 'static>(
	cancelled: &mut impl FnMut() -> bool,
) -> Result<(), PlanError<E>> {
	if cancelled() {
		Err(PlanError::Cancelled)
	} else {
		Ok(())
	}
}
