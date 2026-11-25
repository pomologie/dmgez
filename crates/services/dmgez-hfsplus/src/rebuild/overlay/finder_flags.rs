use std::{collections::BTreeMap, error::Error as StdError};

use dmgez_config::{
	VirtualPath,
	project_edit::{ContentChanges, FieldChange},
};
use dmgez_finder::finder_info::DisplayFlags;
use hfsplus_core::ReadAt;

use super::super::{FINDER_INFO_BYTES, Kind, LinkIdentity, Node};
use super::{OverlayError, paths::Route};

mod signing;

type FinderInfo = [u8; FINDER_INFO_BYTES];
pub(super) type FlagPlan = BTreeMap<usize, FinderInfo>;

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
enum FlagTarget<'image> {
	Path(&'image VirtualPath),
	Inode(LinkIdentity),
}

impl<'image> FlagTarget<'image> {
	fn new<S>(path: &'image VirtualPath, kind: &Kind<'_, S>) -> Self {
		match kind {
			Kind::File {
				inode: Some(inode), ..
			} => Self::Inode(*inode),
			_ => Self::Path(path),
		}
	}
}

#[derive(Default)]
pub(super) struct FlagEdits<'image> {
	targets: BTreeMap<&'image VirtualPath, usize>,
	values: BTreeMap<FlagTarget<'image>, FinderInfo>,
}

impl<'image> FlagEdits<'image> {
	pub(super) fn record<S>(
		&mut self,
		path: &'image VirtualPath,
		entry: &Node<'_, S>,
		index: usize,
		fields: &ContentChanges,
	) {
		let flags = DisplayFlags {
			hidden: display_override(fields.hidden),
			hide_extension: display_override(fields.hide_extension),
		};
		if flags != DisplayFlags::default() {
			self.targets.entry(path).or_insert(index);
			let info = self
				.values
				.entry(FlagTarget::new(path, &entry.kind))
				.or_insert(entry.metadata.finder_info);
			flags.apply(info);
		}
	}

	pub(super) fn targets(&self, path: &VirtualPath) -> bool {
		self.targets.contains_key(path)
	}

	pub(super) fn finish<S: ReadAt>(
		self,
		entries: &'image BTreeMap<VirtualPath, Node<'_, S>>,
		routes: &[Route],
		additions: &[(VirtualPath, Node<'_, S>)],
	) -> Result<FlagPlan, OverlayError>
	where
		S::Error: StdError + 'static,
	{
		let mut result = BTreeMap::new();
		if self.values.is_empty() {
			return Ok(result);
		}
		let mut signing = signing::SignedBundles::new(entries, routes, additions)?;
		for (index, ((path, entry), route)) in entries.iter().zip(routes).enumerate() {
			let Some(destination) = route.path(path) else {
				if let Some(&index) = self.targets.get(path) {
					return Err(OverlayError::ConflictingEdit { index });
				}
				continue;
			};
			if let Some(&info) = self.values.get(&FlagTarget::new(path, &entry.kind))
				&& entry.metadata.finder_info != info
			{
				signing.check_target(path, destination, entry)?;
				result.insert(index, info);
			}
		}
		Ok(result)
	}
}

const fn display_override(change: FieldChange<bool>) -> Option<bool> {
	match change {
		FieldChange::Keep => None,
		FieldChange::Set(value) => Some(value),
		FieldChange::Clear => Some(false),
	}
}
