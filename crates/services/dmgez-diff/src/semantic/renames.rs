use std::{collections::BTreeMap, error::Error as StdError, num::NonZeroU32};

use dmgez_vfs::volume::{ForkKind, NodeHandle, NodeKind, VolumeBackend, VolumeView};

use crate::Side;

use super::{
	Error, RenameCandidate,
	catalog::Catalog,
	path_matching::Pairs,
	payloads::{ForkDigest, Payloads, missing_data},
};

#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct FileFingerprint {
	data: ForkDigest,
	resource: Option<ForkDigest>,
}

type File<'a, Id> = (&'a [String], &'a NodeHandle<Id>);
// A repeated fingerprint loses its unique path even if a third file matches it.
type FingerprintIndex<'a> = BTreeMap<FileFingerprint, Option<&'a [String]>>;

pub(super) fn infer<'catalog, L: VolumeBackend, R: VolumeBackend>(
	left: (&mut VolumeView<L>, &'catalog Catalog<L::Id>),
	right: (&mut VolumeView<R>, &'catalog Catalog<R::Id>),
	pairs: &Pairs<'catalog>,
	payloads: &mut Payloads<impl FnMut() -> bool>,
) -> Result<Vec<RenameCandidate>, Error<L::Error, R::Error>> {
	let (left_view, left_catalog) = left;
	let (right_view, right_catalog) = right;
	let left_files = unmatched(
		left_catalog,
		|path| {
			pairs
				.right(path)
				.is_some_and(|paired| right_catalog.contains_key(paired))
		},
		payloads,
	)?;
	let right_files = unmatched(
		right_catalog,
		|path| {
			pairs
				.left(path)
				.is_some_and(|paired| left_catalog.contains_key(paired))
		},
		payloads,
	)?;
	if left_files.is_empty() || right_files.is_empty() {
		return Ok(Vec::new());
	}
	let left = index(Side::Left, left_files, |node, kind, path| {
		payloads.fingerprint_left::<L, R::Error>((left_view, node), kind, path)
	})?;
	let right = index(Side::Right, right_files, |node, kind, path| {
		payloads.fingerprint_right::<L::Error, R>((right_view, node), kind, path)
	})?;

	let mut candidates = Vec::new();
	for (fingerprint, from) in left {
		payloads.poll()?;
		if let Some(from) = from
			&& let Some(Some(to)) = right.get(&fingerprint)
		{
			candidates.push(RenameCandidate::new(from, to));
		}
	}
	candidates.sort_unstable_by(|left, right| left.from.cmp(&right.from));
	Ok(candidates)
}

fn unmatched<'a, Id, L: StdError + 'static, R: StdError + 'static>(
	source: &'a Catalog<Id>,
	is_matched: impl Fn(&'a [String]) -> bool,
	payloads: &mut Payloads<impl FnMut() -> bool>,
) -> Result<Vec<File<'a, Id>>, Error<L, R>> {
	let mut files = Vec::new();
	for (path, entry) in source {
		payloads.poll()?;
		if entry.metadata.kind == NodeKind::RegularFile
			&& entry
				.metadata
				.hard_link
				.as_ref()
				.is_none_or(|link| link.count == NonZeroU32::MIN)
			&& !is_matched(path)
		{
			files.push((path.as_slice(), &entry.node));
		}
	}
	Ok(files)
}

fn index<'a, Id, L: StdError + 'static, R: StdError + 'static>(
	side: Side,
	files: Vec<File<'a, Id>>,
	mut fingerprint: impl FnMut(
		&NodeHandle<Id>,
		ForkKind,
		&[String],
	) -> Result<Option<ForkDigest>, Error<L, R>>,
) -> Result<FingerprintIndex<'a>, Error<L, R>> {
	let mut index = BTreeMap::new();
	for (path, node) in files {
		let data =
			fingerprint(node, ForkKind::Data, path)?.ok_or_else(|| missing_data(side, path))?;
		let resource = fingerprint(node, ForkKind::Resource, path)?;
		if data.length == 0 && resource.is_none_or(|fork| fork.length == 0) {
			continue;
		}
		index
			.entry(FileFingerprint { data, resource })
			.and_modify(|unique| *unique = None)
			.or_insert(Some(path));
	}
	Ok(index)
}
