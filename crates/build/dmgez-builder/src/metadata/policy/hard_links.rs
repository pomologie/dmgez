use std::mem::size_of;

use super::Error;
use crate::manifest::{Entry, EntryKind, Manifest, SourceIdentity};

/// Included hard-link relationships indexed by verified source identity.
#[derive(Debug)]
pub struct HardLinks {
	identities: Vec<SourceIdentity>,
}

impl HardLinks {
	/// Find relationships between included regular files and symbolic links.
	/// A repeated source with one native link remains a set of independent
	/// copies. Directory link counts never identify file hard links.
	///
	/// # Errors
	/// Returns an error if the identity index cannot fit the supplied budget.
	pub fn discover(manifest: &Manifest, max_bytes: usize) -> Result<Self, Error> {
		let identities = || manifest.entries().iter().filter_map(candidate);
		let candidates = identities().count();
		if candidates > max_bytes / size_of::<SourceIdentity>() {
			return Err(Error::Budget);
		}
		let mut entries = Vec::new();
		entries.try_reserve_exact(candidates)?;
		if entries.capacity() > max_bytes / size_of::<SourceIdentity>() {
			return Err(Error::Budget);
		}
		entries.extend(identities());
		entries.sort_unstable();
		let mut groups = 0;
		let mut start = 0;
		while let Some(&identity) = entries.get(start) {
			let count = entries[start..].partition_point(|entry| *entry == identity);
			if count > 1 {
				entries[groups] = identity;
				groups += 1;
			}
			start += count;
		}
		entries.truncate(groups);
		Ok(Self {
			identities: entries,
		})
	}

	/// Return the identity of this entry's included group, when any.
	#[must_use]
	pub fn group_for(&self, entry: &Entry) -> Option<SourceIdentity> {
		let candidate = candidate(entry)?;
		self.identities
			.binary_search(&candidate)
			.ok()
			.map(|_| candidate)
	}

	/// Retained allocation charged to the builder's metadata budget.
	#[must_use]
	pub fn accounted_bytes(&self) -> usize {
		self.identities.capacity() * size_of::<SourceIdentity>()
	}
}

fn candidate(entry: &Entry) -> Option<SourceIdentity> {
	let metadata = entry.source.as_ref()?.metadata();
	(matches!(entry.kind, EntryKind::File | EntryKind::Symlink(_)) && metadata.link_count > 1)
		.then_some(metadata.identity)
}
