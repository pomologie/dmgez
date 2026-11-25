use std::{
	collections::{BTreeMap, BTreeSet},
	error::Error as StdError,
};

use dmgez_config::VirtualPath;
use dmgez_macho::{
	bundle::{
		has_app_suffix, inside_signed_bundle, is_signing_attribute, signed_bundle_root,
		sort_signed_bundle_roots,
	},
	has_or_may_have_signature_at,
};
use hfsplus_core::{ByteOffset, ReadAt};

use super::{Kind, LinkIdentity, Node, OverlayError, Route};

pub(super) struct SignedBundles<'image> {
	original: Vec<&'image str>,
	edited: Vec<&'image str>,
	inspected_inodes: BTreeSet<LinkIdentity>,
}

impl<'image> SignedBundles<'image> {
	pub(super) fn new<S>(
		entries: &'image BTreeMap<VirtualPath, Node<'_, S>>,
		routes: &'image [Route],
		additions: &'image [(VirtualPath, Node<'_, S>)],
	) -> Result<Self, OverlayError> {
		let original = roots(entries.iter())?;
		let edited = roots(
			entries
				.iter()
				.zip(routes)
				.filter_map(|((path, node), route)| Some((route.path(path)?, node)))
				.chain(additions.iter().map(|(path, node)| (path, node))),
		)?;
		Ok(Self {
			original,
			edited,
			inspected_inodes: BTreeSet::new(),
		})
	}

	pub(super) fn check_target<S: ReadAt>(
		&mut self,
		original: &VirtualPath,
		destination: &VirtualPath,
		entry: &Node<'_, S>,
	) -> Result<(), OverlayError>
	where
		S::Error: StdError + 'static,
	{
		for (path, roots) in [(original, &self.original), (destination, &self.edited)] {
			if path
				.components()
				.any(|name| has_app_suffix(name.as_bytes()))
				|| inside_signed_bundle(path.as_str(), roots)
			{
				return Err(OverlayError::FinderSignedTarget { path: path.clone() });
			}
		}
		if entry
			.attributes
			.iter()
			.any(|(name, _)| is_signing_attribute(name.as_bytes()))
		{
			return Err(OverlayError::FinderSignedTarget {
				path: original.clone(),
			});
		}
		if let Kind::File { file, inode } = &entry.kind
			&& inode.is_none_or(|inode| self.inspected_inodes.insert(inode))
		{
			let signed = has_or_may_have_signature_at(file.data.len().0, |offset, output| {
				file.data.read_exact_at(ByteOffset(offset), output)
			})
			.map_err(|source| OverlayError::FinderSigningProbe {
				path: original.clone(),
				reason: source.to_string(),
			})?;
			if signed {
				return Err(OverlayError::FinderSignedTarget {
					path: original.clone(),
				});
			}
		}
		Ok(())
	}
}

fn roots<'image, 'volume: 'image, S: 'volume>(
	entries: impl Iterator<Item = (&'image VirtualPath, &'image Node<'volume, S>)>,
) -> Result<Vec<&'image str>, OverlayError> {
	let mut roots = Vec::new();
	for (path, node) in entries {
		if matches!(node.kind, Kind::File { .. })
			&& let Some(root) = signed_bundle_root(path.as_str())
		{
			roots.try_reserve(1)?;
			roots.push(root);
		}
	}
	sort_signed_bundle_roots(&mut roots);
	Ok(roots)
}
