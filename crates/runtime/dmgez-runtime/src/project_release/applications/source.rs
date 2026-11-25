use std::{io, iter, path::Path};

use dmgez_builder::{
	manifest::{Entry, EntryKind, Limits, SourceSnapshot, inspect_tree},
	planning::PreparedBuild,
};
use dmgez_config::VirtualPath;

use crate::project_release::StageResult;

use super::inventory::is_bundle_name;

const PATH_SEPARATOR: char = '/';
const INCOMPLETE_APP: &str = "release application does not match one complete host source tree";
const MODIFIED_APP: &str = "release application includes project metadata overrides";
#[cfg(feature = "finder-write")]
const GENERATED_IN_APP: &str = "release application contains generated Finder metadata";

#[cfg(test)]
mod tests;

pub(super) fn prepare(
	prepared: &PreparedBuild,
	mut verify: impl FnMut(&Path) -> StageResult,
) -> StageResult<Vec<VirtualPath>> {
	check_generated_assets(prepared)?;
	let entries = prepared.manifest().entries();
	let mut applications = Vec::new();
	for root in entries.iter().filter(|entry| is_application_root(entry)) {
		let bundle = Bundle::new(root, entries);
		let source = bundle.bind(prepared)?;
		let checked = verify(source.path());
		// Recheck even when the native tool rejected the observed bundle.
		bundle.validate_sources()?;
		checked?;
		applications.try_reserve(1)?;
		applications.push(root.destination.clone());
	}
	Ok(applications)
}

struct Bundle<'manifest> {
	root: &'manifest Entry,
	descendants: &'manifest [Entry],
}

impl<'manifest> Bundle<'manifest> {
	fn new(root: &'manifest Entry, manifest: &'manifest [Entry]) -> Self {
		let prefix = format!("{}{PATH_SEPARATOR}", root.destination);
		let start = manifest.partition_point(|entry| entry.destination.as_str() < prefix.as_str());
		let descendants = &manifest[start..];
		let count =
			descendants.partition_point(|entry| entry.destination.as_str().starts_with(&prefix));
		Self {
			root,
			descendants: &descendants[..count],
		}
	}

	fn entries(&self) -> impl Iterator<Item = &'manifest Entry> {
		iter::once(self.root).chain(self.descendants)
	}

	fn bind(&self, prepared: &PreparedBuild) -> StageResult<&'manifest SourceSnapshot> {
		let root = self.source(self.root)?;
		for entry in self.entries() {
			let source = self.source(entry)?;
			if entry.finder.hidden.is_some()
				|| entry.finder.hide_extension.is_some()
				|| entry.finder.custom_icon.is_some()
				|| prepared.metadata_sidecar_for(source).is_some()
			{
				return Err(invalid_entry(MODIFIED_APP, &entry.destination).into());
			}
			source.validate()?;
		}
		let visited = u64::try_from(self.descendants.len())? + 1;
		let root_depth = u64::try_from(self.root.destination.components().count())?;
		let profile = prepared.profile().limits();
		let scratch_budget = profile.memory_bytes - profile.max_metadata_bytes;
		let observed = inspect_tree(
			root,
			&self.root.destination,
			Limits {
				max_entries: visited + root_depth - 1,
				max_visited: visited,
				max_metadata_bytes: u64::try_from(profile.max_metadata_bytes.min(scratch_budget))?,
				..Limits::default()
			},
			|| {
				prepared
					.profile()
					.write_options()
					.cancellation
					.is_cancelled()
			},
		)
		.map_err(|error| {
			io::Error::other(format!(
				"{INCOMPLETE_APP} at {:?}: {error}",
				self.root.destination
			))
		})?;
		let observed_root = observed
			.entries()
			.binary_search_by(|entry| entry.destination.cmp(&self.root.destination));
		let Ok(observed_root) = observed_root else {
			return Err(invalid_entry(INCOMPLETE_APP, &self.root.destination).into());
		};
		let observed_bundle = Bundle::new(&observed.entries()[observed_root], observed.entries());
		if observed_bundle.descendants.len() != self.descendants.len()
			|| !self
				.entries()
				.zip(observed_bundle.entries())
				.all(|(planned, actual)| same_entry(planned, actual))
		{
			return Err(invalid_entry(INCOMPLETE_APP, &self.root.destination).into());
		}
		Ok(root)
	}

	fn validate_sources(&self) -> StageResult {
		for entry in self.entries() {
			self.source(entry)?.validate()?;
		}
		Ok(())
	}

	fn source(&self, entry: &'manifest Entry) -> io::Result<&'manifest SourceSnapshot> {
		entry
			.source
			.as_ref()
			.ok_or_else(|| invalid_entry(INCOMPLETE_APP, &entry.destination))
	}
}

fn is_application_root(entry: &Entry) -> bool {
	let mut components = entry.destination.as_str().rsplit(PATH_SEPARATOR);
	matches!(entry.kind, EntryKind::Directory)
		&& components.next().is_some_and(is_bundle_name)
		&& !components.any(is_bundle_name)
}

fn same_entry(planned: &Entry, observed: &Entry) -> bool {
	if planned.destination != observed.destination || !same_kind(&planned.kind, &observed.kind) {
		return false;
	}
	let (Some(planned), Some(observed)) = (&planned.source, &observed.source) else {
		return false;
	};
	// Directory enumeration and signature verification may update access time.
	let mut metadata = *observed.metadata();
	metadata.accessed = planned.metadata().accessed;
	planned.path() == observed.path() && *planned.metadata() == metadata
}

fn same_kind(planned: &EntryKind, observed: &EntryKind) -> bool {
	match (planned, observed) {
		(EntryKind::Directory, EntryKind::Directory) | (EntryKind::File, EntryKind::File) => true,
		(EntryKind::Symlink(expected), EntryKind::Symlink(actual)) => expected == actual,
		_ => false,
	}
}

fn check_generated_assets(prepared: &PreparedBuild) -> io::Result<()> {
	#[cfg(feature = "finder-write")]
	for path in prepared
		.finder_assets()
		.iter()
		.map(|asset| &asset.path)
		.chain(prepared.finder_directories())
		.chain(
			prepared
				.deferred_finder_stores()
				.iter()
				.map(|store| store.path()),
		) {
		if path.components().any(is_bundle_name) {
			return Err(invalid_entry(GENERATED_IN_APP, path));
		}
	}
	#[cfg(not(feature = "finder-write"))]
	let _ = prepared;
	Ok(())
}

fn invalid_entry(reason: &str, path: &VirtualPath) -> io::Error {
	io::Error::new(io::ErrorKind::InvalidData, format!("{reason} at {path:?}"))
}
