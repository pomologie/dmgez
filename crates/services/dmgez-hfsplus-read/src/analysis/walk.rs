use std::{
	cmp::{Ordering, Reverse},
	collections::{BTreeMap, BinaryHeap},
	num::NonZeroUsize,
};

use dmgez_vfs::{LogicalDisk, volume::NodeKind};
use hfsplus_core::ReadAt;
use hfsplus_format::volume::CatalogId;
use hfsplus_reader::{
	catalog::{Catalog, Entry, EntryKind, HardLinkError},
	decmpfs::LogicalDataError,
};

use crate::browse::{WindowSource, node_kind};

use super::{Content, Error, File, Limit, Limits, SizeUnavailable, UnavailableSize};

const PAGE_ENTRIES: usize = 16;

pub(super) fn summarize<S: LogicalDisk>(
	catalog: &Catalog<'_, WindowSource<'_, S>>,
	limits: Limits,
) -> Result<Content, Error> {
	let mut walk = Walk {
		content: Content::default(),
		unavailable: BTreeMap::new(),
		largest: BinaryHeap::new(),
		limits,
	};
	walk.largest.try_reserve_exact(limits.largest_files)?;
	let mut ancestors = Vec::new();
	ancestors.try_reserve_exact(usize::from(limits.max_depth))?;
	walk.directory(catalog, catalog.root()?.metadata.id, &mut ancestors)?;
	Ok(walk.finish())
}

struct Walk {
	content: Content,
	unavailable: BTreeMap<SizeUnavailable, u64>,
	largest: BinaryHeap<Reverse<Ranked>>,
	limits: Limits,
}

impl Walk {
	fn directory<S: LogicalDisk>(
		&mut self,
		catalog: &Catalog<'_, WindowSource<'_, S>>,
		id: CatalogId,
		ancestors: &mut Vec<Vec<u16>>,
	) -> Result<(), Error> {
		let mut directory = catalog.read_dir(id)?;
		loop {
			let page = directory.next_page(self.page_entries())?;
			for entry in page.entries {
				if self.content.entries == self.limits.max_entries {
					self.record_limit(Limit::Entries);
					return Ok(());
				}
				self.content.entries += 1;
				if matches!(entry.kind, EntryKind::Directory { .. }) {
					self.content.directories += 1;
					if ancestors.len() + 1 < usize::from(self.limits.max_depth) {
						ancestors.push(entry.name);
						self.directory(catalog, entry.metadata.id, ancestors)?;
						ancestors.pop();
						if self.content.limits_reached.contains(&Limit::Entries) {
							return Ok(());
						}
					} else {
						let mut child = catalog.read_dir(entry.metadata.id)?;
						let first = child.next_page(NonZeroUsize::MIN)?;
						if !first.entries.is_empty() {
							self.record_limit(Limit::Depth);
						}
					}
					continue;
				}
				match node_kind(&entry) {
					NodeKind::Symlink => self.content.symlinks += 1,
					_ => self.file(catalog, entry, ancestors)?,
				}
			}
			if page.complete {
				return Ok(());
			}
		}
	}

	fn page_entries(&self) -> NonZeroUsize {
		let remaining = self.limits.max_entries - self.content.entries;
		// Allow one extra entry to establish that an exact ceiling omitted data.
		let needed = remaining.saturating_add(1).min(PAGE_ENTRIES as u64) as usize;
		let ceiling = self.limits.source.catalog.max_page_entries;
		NonZeroUsize::new(ceiling.min(needed)).unwrap_or(NonZeroUsize::MIN)
	}

	fn file<S: LogicalDisk>(
		&mut self,
		catalog: &Catalog<'_, WindowSource<'_, S>>,
		entry: Entry,
		ancestors: &[Vec<u16>],
	) -> Result<(), Error> {
		let id = entry.metadata.id;
		let file = match catalog.regular_file(id, self.limits.source.hard_links) {
			Ok(Some(file)) => file,
			Ok(None) => return Err(Error::MissingFile(id)),
			Err(HardLinkError::DirectoryLink(_)) => {
				self.content.other_entries += 1;
				self.record_unavailable(SizeUnavailable::DirectoryLink);
				return Ok(());
			}
			Err(HardLinkError::NotRegular(_)) => {
				self.content.other_entries += 1;
				return Ok(());
			}
			Err(error) => return Err(error.into()),
		};
		self.content.regular_files += 1;
		let logical_bytes = match file.open_logical_data(self.limits.source.logical_data) {
			Ok(fork) => fork.len().0,
			Err(LogicalDataError::CompressionDisabled) => {
				self.record_unavailable(SizeUnavailable::CompressionDisabled);
				return Ok(());
			}
			Err(LogicalDataError::UnsupportedCompression(_)) => {
				self.record_unavailable(SizeUnavailable::UnsupportedCompression);
				return Ok(());
			}
			Err(LogicalDataError::Limit(_)) => {
				self.record_unavailable(SizeUnavailable::CompressionLimit);
				return Ok(());
			}
			Err(error) => return Err(error.into()),
		};
		self.content.sized_files += 1;
		if self.largest.len() == self.limits.largest_files
			&& let Some(Reverse(worst)) = self.largest.peek()
			&& rank(logical_bytes, id.0) <= worst.rank()
		{
			return Ok(());
		}

		let mut path = Vec::new();
		path.try_reserve_exact(ancestors.len() + 1)?;
		for ancestor in ancestors {
			let mut copied = Vec::new();
			copied.try_reserve_exact(ancestor.len())?;
			copied.extend_from_slice(ancestor);
			path.push(copied);
		}
		path.push(entry.name);
		let candidate = Ranked(File {
			path,
			catalog_id: id.0,
			inode_id: file.inode_id().0,
			logical_bytes,
		});
		if self.largest.len() == self.limits.largest_files {
			self.largest.pop();
		}
		self.largest.push(Reverse(candidate));
		Ok(())
	}

	fn record_limit(&mut self, limit: Limit) {
		if !self.content.limits_reached.contains(&limit) {
			self.content.limits_reached.push(limit);
		}
	}

	fn record_unavailable(&mut self, reason: SizeUnavailable) {
		*self.unavailable.entry(reason).or_default() += 1;
	}

	fn finish(mut self) -> Content {
		self.content.complete =
			self.content.limits_reached.is_empty() && self.unavailable.is_empty();
		self.content.unavailable_sizes = self
			.unavailable
			.into_iter()
			.map(|(reason, files)| UnavailableSize { reason, files })
			.collect();
		self.content.largest_files = self
			.largest
			.into_sorted_vec()
			.into_iter()
			.map(|Reverse(Ranked(file))| file)
			.collect();
		self.content
	}
}

struct Ranked(File);

impl Ranked {
	fn rank(&self) -> (u64, Reverse<u32>) {
		rank(self.0.logical_bytes, self.0.catalog_id)
	}
}

fn rank(logical_bytes: u64, catalog_id: u32) -> (u64, Reverse<u32>) {
	(logical_bytes, Reverse(catalog_id))
}

impl PartialEq for Ranked {
	fn eq(&self, other: &Self) -> bool {
		self.rank() == other.rank()
	}
}

impl Eq for Ranked {}

impl PartialOrd for Ranked {
	fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
		Some(self.cmp(other))
	}
}

impl Ord for Ranked {
	fn cmp(&self, other: &Self) -> Ordering {
		self.rank().cmp(&other.rank())
	}
}
