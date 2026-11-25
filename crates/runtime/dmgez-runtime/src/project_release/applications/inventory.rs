use std::{collections::BTreeSet, hash::Hash, io};

use dmgez_config::VirtualPath;
use dmgez_operations::extract::{EntryId, NameLimits, NamePlan, NameProfile, preflight_names};
use dmgez_vfs::volume::{NodeKind, VolumeBackend, VolumeView};

use crate::project_release::StageResult;

use super::PATH_SEPARATOR;

const APP_EXTENSION: &str = "app";
const EXTENSION_SEPARATOR: char = '.';

pub(super) fn match_applications<B: VolumeBackend<Error: Send + Sync>>(
	view: &mut VolumeView<B>,
	expected: &[VirtualPath],
) -> StageResult
where
	B::Id: Eq + Hash,
{
	let root = view.root()?;
	let names = preflight_names(
		view,
		&root,
		NameProfile::PosixUtf8,
		NameLimits::default(),
		|| false,
	)?;
	let mut remaining: BTreeSet<_> = expected.iter().map(VirtualPath::as_str).collect();
	let mut inside = Vec::new();
	inside.try_reserve_exact(names.entries().len())?;
	for (index, entry) in names.entries().iter().enumerate() {
		let nested = entry.parent.is_some_and(|parent| inside[parent.index()]);
		let bundle = entry.kind == NodeKind::Directory && is_bundle_name(entry.name.as_str());
		if bundle && !nested {
			let path = path_to(&names, index)?;
			if !remaining.remove(path.as_str()) {
				return Err(io::Error::new(
					io::ErrorKind::InvalidData,
					format!("unexpected staged application {path:?}"),
				)
				.into());
			}
		}
		inside.push(nested || bundle);
	}
	if let Some(missing) = remaining.first() {
		return Err(io::Error::new(
			io::ErrorKind::InvalidData,
			format!("missing staged application {missing:?}"),
		)
		.into());
	}
	Ok(())
}

pub(super) fn is_bundle_name(name: &str) -> bool {
	name.rsplit_once(EXTENSION_SEPARATOR)
		.is_some_and(|(stem, extension)| {
			!stem.is_empty() && extension.eq_ignore_ascii_case(APP_EXTENSION)
		})
}

fn path_to<Id>(names: &NamePlan<Id>, index: usize) -> StageResult<String> {
	let mut current = Some(index);
	let mut components = Vec::new();
	while let Some(index) = current {
		let entry = &names.entries()[index];
		components.try_reserve(1)?;
		components.push(entry.name.as_str());
		current = entry.parent.map(EntryId::index);
	}
	let mut path = String::new();
	for name in components.into_iter().rev() {
		path.try_reserve(PATH_SEPARATOR.len_utf8() + name.len())?;
		path.push(PATH_SEPARATOR);
		path.push_str(name);
	}
	Ok(path)
}
