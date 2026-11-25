use std::{
	collections::{BTreeMap, HashSet, btree_map::Entry as MapEntry},
	error::Error as StdError,
	hash::Hash,
	num::NonZeroU16,
};

use dmgez_vfs::volume::{
	Metadata, NodeHandle, NodeKind, RawName, StorageId, ViewError, VolumeBackend, VolumeView,
};
use thiserror::Error;

use super::Limits;

const DIRECTORY_PAGE_ENTRIES: usize = 64;
const ROOT_PATH_BYTES: usize = 1;
const SEPARATOR_BYTES: usize = 1;
const NUL: char = '\0';

#[cfg(test)]
mod tests;

/// Failure to collect a complete, unambiguous volume catalog under the policy.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum CatalogError<E: StdError + 'static> {
	#[error("could not read the filesystem catalog")]
	View {
		#[source]
		source: ViewError<E>,
	},
	#[error("catalog exceeds its {limit} entry limit")]
	EntryLimit { limit: usize },
	#[error("catalog exceeds its {limit} component depth limit")]
	DepthLimit { limit: usize },
	#[error("catalog exceeds its {limit} aggregate path bytes limit")]
	PathBytesLimit { limit: usize },
	#[error("filesystem name cannot be compared as exact stored text")]
	UnsupportedName,
	#[error("filesystem catalog repeats an exact stored path")]
	DuplicatePath,
	#[error("filesystem catalog reaches a directory more than once")]
	RepeatedDirectory,
	#[error("filesystem directory scan did not advance")]
	StalledDirectory,
	#[error("filesystem catalog comparison cancelled")]
	Cancelled,
}

impl<E: StdError + 'static> From<ViewError<E>> for CatalogError<E> {
	fn from(source: ViewError<E>) -> Self {
		Self::View { source }
	}
}

pub(super) struct Entry<Id> {
	pub(super) node: NodeHandle<Id>,
	pub(super) metadata: Metadata<StorageId>,
}

pub(super) type Catalog<Id> = BTreeMap<Vec<String>, Entry<Id>>;

struct CatalogNode<Id> {
	path: Vec<String>,
	path_bytes: usize,
	node: NodeHandle<Id>,
}

#[derive(Eq, Hash, PartialEq)]
enum DirectoryId<Id> {
	Numeric(u64),
	Native(Id),
}

pub(super) fn collect<B: VolumeBackend>(
	view: &mut VolumeView<B>,
	limits: Limits,
	cancelled: &mut impl FnMut() -> bool,
) -> Result<Catalog<B::Id>, CatalogError<B::Error>>
where
	B::Id: Eq + Hash,
{
	check_cancelled(cancelled)?;
	let mut budget = Budget::new(limits)?;
	let root = view.root()?;
	let metadata = view.stat(&root)?;
	let mut visited = HashSet::from([directory_id(view, &root)?]);
	let mut entries = BTreeMap::from([(
		Vec::new(),
		Entry {
			node: root.clone(),
			metadata,
		},
	)]);
	let mut pending = vec![CatalogNode {
		path: Vec::new(),
		path_bytes: ROOT_PATH_BYTES,
		node: root,
	}];
	while let Some(directory) = pending.pop() {
		check_cancelled(cancelled)?;
		for CatalogNode {
			path,
			path_bytes,
			node,
		} in children(view, &directory, &mut budget, cancelled)?
		{
			check_cancelled(cancelled)?;
			let MapEntry::Vacant(entry) = entries.entry(path) else {
				return Err(CatalogError::DuplicatePath);
			};
			let metadata = view.stat(&node)?;
			if metadata.kind == NodeKind::Directory {
				if !visited.insert(directory_id(view, &node)?) {
					return Err(CatalogError::RepeatedDirectory);
				}
				pending.push(CatalogNode {
					path: entry.key().clone(),
					path_bytes,
					node: node.clone(),
				});
			}
			entry.insert(Entry { node, metadata });
		}
	}
	Ok(entries)
}

fn directory_id<B: VolumeBackend>(
	view: &VolumeView<B>,
	node: &NodeHandle<B::Id>,
) -> Result<DirectoryId<B::Id>, CatalogError<B::Error>> {
	Ok(match view.numeric_id(node)? {
		Some(id) => DirectoryId::Numeric(id),
		None => DirectoryId::Native(node.native_id()),
	})
}

fn children<B: VolumeBackend>(
	view: &mut VolumeView<B>,
	parent: &CatalogNode<B::Id>,
	budget: &mut Budget,
	cancelled: &mut impl FnMut() -> bool,
) -> Result<Vec<CatalogNode<B::Id>>, CatalogError<B::Error>> {
	let mut scan = view.read_dir(&parent.node)?;
	let mut children = Vec::new();
	loop {
		check_cancelled(cancelled)?;
		let remaining = budget.remaining_entries();
		// Request one extra entry to distinguish a full catalog from a truncated one.
		let page_size = remaining.saturating_add(1).min(DIRECTORY_PAGE_ENTRIES) as u16;
		let page = scan.next_page(NonZeroU16::new(page_size).unwrap_or(NonZeroU16::MIN))?;
		if page.entries.len() > remaining {
			return Err(CatalogError::EntryLimit {
				limit: budget.limits.max_entries,
			});
		}
		if page.entries.is_empty() && !page.complete {
			return Err(CatalogError::StalledDirectory);
		}
		if !page.entries.is_empty() && parent.path.len() >= budget.limits.max_depth {
			return Err(CatalogError::DepthLimit {
				limit: budget.limits.max_depth,
			});
		}
		for entry in page.entries {
			check_cancelled(cancelled)?;
			let (name, path_bytes) = budget.charge_name(parent, entry.name)?;
			let mut path = parent.path.clone();
			path.push(name);
			children.push(CatalogNode {
				path,
				path_bytes,
				node: entry.node,
			});
		}
		if page.complete {
			return Ok(children);
		}
	}
}

struct Budget {
	limits: Limits,
	entries: usize,
	path_bytes: usize,
}

impl Budget {
	fn new<E: StdError + 'static>(limits: Limits) -> Result<Self, CatalogError<E>> {
		if limits.max_entries == 0 {
			return Err(CatalogError::EntryLimit {
				limit: limits.max_entries,
			});
		}
		if limits.max_path_bytes < ROOT_PATH_BYTES {
			return Err(CatalogError::PathBytesLimit {
				limit: limits.max_path_bytes,
			});
		}
		Ok(Self {
			limits,
			entries: 1,
			path_bytes: ROOT_PATH_BYTES,
		})
	}

	fn remaining_entries(&self) -> usize {
		self.limits.max_entries - self.entries
	}

	fn charge_name<Id, E: StdError + 'static>(
		&mut self,
		parent: &CatalogNode<Id>,
		name: RawName,
	) -> Result<(String, usize), CatalogError<E>> {
		let limit = self.limits.max_path_bytes;
		let limit_error = || CatalogError::PathBytesLimit { limit };
		let remaining = limit - self.path_bytes;
		let separator = if parent.path.is_empty() {
			0
		} else {
			SEPARATOR_BYTES
		};
		let prefix = parent
			.path_bytes
			.checked_add(separator)
			.ok_or_else(limit_error)?;
		let name_allowance = remaining.checked_sub(prefix).ok_or_else(limit_error)?;
		let name = match name {
			RawName::Utf8(text) => text,
			RawName::Utf16(units) => {
				// Lossless UTF-8 consumes at least as many bytes as UTF-16 units.
				if units.len() > name_allowance {
					return Err(limit_error());
				}
				String::from_utf16(&units).map_err(|_| CatalogError::UnsupportedName)?
			}
			_ => return Err(CatalogError::UnsupportedName),
		};
		if name.is_empty() || name.contains(NUL) {
			return Err(CatalogError::UnsupportedName);
		}
		if name.len() > name_allowance {
			return Err(limit_error());
		}
		let path_bytes = prefix + name.len();
		self.path_bytes += path_bytes;
		self.entries += 1;
		Ok((name, path_bytes))
	}
}

fn check_cancelled<E: StdError + 'static>(
	cancelled: &mut impl FnMut() -> bool,
) -> Result<(), CatalogError<E>> {
	if cancelled() {
		return Err(CatalogError::Cancelled);
	}
	Ok(())
}
