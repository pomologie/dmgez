//! Bounded comparison of visible paths and supported objects in two volume views.

use std::{error::Error as StdError, hash::Hash};

use dmgez_vfs::volume::{NodeKind, ViewError, VolumeBackend, VolumeView};
use thiserror::Error;

use crate::{SCHEMA_VERSION, Side};

mod catalog;
mod finder_info;
#[cfg(feature = "finder-layout")]
mod finder_layout;
mod links;
mod object;
mod path_matching;
mod payloads;
#[cfg(feature = "content-hash")]
mod renames;
mod report;
mod xattrs;

#[cfg(test)]
mod tests;

pub use catalog::CatalogError;
pub use finder_info::FinderInfoError;
#[cfg(feature = "finder-layout")]
pub use finder_layout::FinderLayoutError;
pub use path_matching::PathMatchError;
pub use report::{
	Change, ChangeKind, ContentComparison, EquivalentPathSpelling, Field, FinderInfoComparison,
	FinderInfoPolicy, Limits, Outcome, PathMatching, Policy, Report, Uncompared, XattrChange,
	XattrPolicy,
};
#[cfg(feature = "content-hash")]
pub use report::{Contents, RenameCandidate, RenameInference};
#[cfg(feature = "finder-layout")]
pub use report::{FinderLayoutPolicy, FinderLayoutUncompared, FinderRecordChange};
pub use xattrs::XattrError;

use catalog::{Entry, collect};
use links::Topology;
use object::{PairedPaths, compare_entry};
use path_matching::{Pairs, native_profile};
use payloads::Payloads;
use xattrs::Xattrs;

/// A stopped comparison does not produce a supported-scope equality result.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error<L: StdError + 'static, R: StdError + 'static> {
	#[error("could not collect the left filesystem catalog")]
	LeftCatalog(#[source] CatalogError<L>),
	#[error("could not collect the right filesystem catalog")]
	RightCatalog(#[source] CatalogError<R>),
	#[error("could not match filesystem paths under the selected policy")]
	PathMatching(#[from] PathMatchError<L, R>),
	#[error("could not read the left filesystem object at {path:?}")]
	LeftObject {
		path: Vec<String>,
		#[source]
		source: ViewError<L>,
	},
	#[error("could not read the right filesystem object at {path:?}")]
	RightObject {
		path: Vec<String>,
		#[source]
		source: ViewError<R>,
	},
	#[error("could not compare the left extended attributes at {path:?}")]
	LeftXattrs {
		path: Vec<String>,
		#[source]
		source: XattrError<L>,
	},
	#[error("could not compare the right extended attributes at {path:?}")]
	RightXattrs {
		path: Vec<String>,
		#[source]
		source: XattrError<R>,
	},
	#[error("could not compare the left FinderInfo at {path:?}")]
	LeftFinderInfo {
		path: Vec<String>,
		#[source]
		source: FinderInfoError<L>,
	},
	#[error("could not compare the right FinderInfo at {path:?}")]
	RightFinderInfo {
		path: Vec<String>,
		#[source]
		source: FinderInfoError<R>,
	},
	#[cfg(feature = "finder-layout")]
	#[error("could not compare the left Finder layout at {path:?}")]
	LeftFinderLayout {
		path: Vec<String>,
		#[source]
		source: FinderLayoutError<L>,
	},
	#[cfg(feature = "finder-layout")]
	#[error("could not compare the right Finder layout at {path:?}")]
	RightFinderLayout {
		path: Vec<String>,
		#[source]
		source: FinderLayoutError<R>,
	},
	#[error("the {side} filesystem object at {path:?} has an unsupported kind")]
	UnsupportedKind { side: Side, path: Vec<String> },
	#[error("the {side} regular file at {path:?} has no logical data fork")]
	MissingDataFork { side: Side, path: Vec<String> },
	#[error(
		"the {side} visible hard-link group at {path:?} disagrees with its admitted link count"
	)]
	LinkCount { side: Side, path: Vec<String> },
	#[error("semantic comparison exhausted its {limit} byte payload budget")]
	ByteLimit { limit: u64 },
	#[error("semantic comparison cancelled")]
	Cancelled,
}

/// Independent opt-in scopes for a semantic comparison.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct Options {
	/// Pair common paths by exact stored spelling or a qualified native profile.
	pub paths: PathMatching,
	/// Compare the supported stored extended attributes.
	pub xattrs: bool,
	/// Compare effective per-object FinderInfo on common paths of the same kind.
	pub finder_info: bool,
	/// Decode matched regular Finder store data forks.
	#[cfg(feature = "finder-layout")]
	pub finder_layout: bool,
	/// Compare admitted regular-file forks under this policy on common paths.
	#[cfg(feature = "content-hash")]
	pub contents: Contents,
	/// Infer candidate pairs among otherwise removed and added regular files.
	#[cfg(feature = "content-hash")]
	pub infer_renames: bool,
}

/// Compares the visible root and descendants of two admitted filesystem views.
///
/// Paths match exact stored Unicode text. Regular file forks and symbolic link
/// targets compare exactly and symbolic links are never followed. Native
/// hard-link IDs are local to each view.
///
/// The caller retains responsibility for validating the source identities
/// before trusting a returned report.
///
/// # Errors
/// Stops on cancellation, budget exhaustion, unavailable object information,
/// or unsupported or inconsistent filesystem records.
pub fn compare<L, R>(
	left: &mut VolumeView<L>,
	right: &mut VolumeView<R>,
	limits: Limits,
	cancelled: impl FnMut() -> bool,
) -> Result<Report, Error<L::Error, R::Error>>
where
	L: VolumeBackend,
	R: VolumeBackend,
	L::Id: Eq + Hash,
	R::Id: Eq + Hash,
{
	compare_with_options(left, right, limits, Options::default(), cancelled)
}

/// Compares the visible trees and the supported stored extended attributes.
///
/// Attributes are matched by exact stored Unicode spelling on each common
/// path. The result lists names and change kinds, and excludes internal
/// records listed in the policy. Matched attribute data shares the byte
/// budget with file forks and symbolic link targets.
///
/// # Errors
/// Stops on the same failures as [`compare`], or if attribute enumeration or
/// matched values cannot be verified within the reported bounds.
pub fn compare_with_xattrs<L, R>(
	left: &mut VolumeView<L>,
	right: &mut VolumeView<R>,
	limits: Limits,
	cancelled: impl FnMut() -> bool,
) -> Result<Report, Error<L::Error, R::Error>>
where
	L: VolumeBackend,
	R: VolumeBackend,
	L::Id: Eq + Hash,
	R::Id: Eq + Hash,
{
	compare_with_options(
		left,
		right,
		limits,
		Options {
			xattrs: true,
			..Options::default()
		},
		cancelled,
	)
}

/// Compares the visible trees, decoding the data forks of matched regular Finder stores.
///
/// The admitted single-leaf stores are compared by reported record facts and
/// identities. Other data forks and all supported resource forks are compared
/// exactly. Both stores share the logical payload budget with other forks,
/// symlink targets, and any enabled extended attributes.
/// # Errors
/// Stops on the same failures as [`compare`] or [`compare_with_xattrs`], or if
/// a matched store or its reported background alias fails the admitted profile.
#[cfg(feature = "finder-layout")]
pub fn compare_with_finder_layout<L, R>(
	left: &mut VolumeView<L>,
	right: &mut VolumeView<R>,
	limits: Limits,
	xattrs: bool,
	cancelled: impl FnMut() -> bool,
) -> Result<Report, Error<L::Error, R::Error>>
where
	L: VolumeBackend,
	R: VolumeBackend,
	L::Id: Eq + Hash,
	R::Id: Eq + Hash,
{
	compare_with_options(
		left,
		right,
		limits,
		Options {
			xattrs,
			finder_layout: true,
			..Options::default()
		},
		cancelled,
	)
}

/// Compares the visible trees under the requested independent scopes.
///
/// FinderInfo compares effective records of same-kind objects on common paths,
/// including the volume root. A pair with at least one present record charges
/// one record length to the shared logical payload budget. A symbolic link is
/// compared as an object and is never followed.
/// With content hashing enabled, the fork comparison mode applies to common
/// regular files. Rename inference uses fork fingerprints on unmatched paths.
/// Both operations consume the same payload budget as the other scopes.
/// Hashes may be reused for effective hard-link storage within each admitted
/// view. The caller validates source identities as with [`compare`].
///
/// Native path matching requires a shared admitted key profile and rejects
/// ambiguous matches. Stored spellings remain available in the report.
///
/// # Errors
/// Stops on the same failures as [`compare`] and on unsupported, unreadable or
/// over-budget records in any enabled scope.
pub fn compare_with_options<L, R>(
	left: &mut VolumeView<L>,
	right: &mut VolumeView<R>,
	limits: Limits,
	options: Options,
	mut cancelled: impl FnMut() -> bool,
) -> Result<Report, Error<L::Error, R::Error>>
where
	L: VolumeBackend,
	R: VolumeBackend,
	L::Id: Eq + Hash,
	R::Id: Eq + Hash,
{
	let native_profile = native_profile(left, right, options.paths, &mut cancelled)?;
	let left_catalog = collect(left, limits, &mut cancelled).map_err(Error::LeftCatalog)?;
	let right_catalog = collect(right, limits, &mut cancelled).map_err(Error::RightCatalog)?;
	let pairs = Pairs::new(
		(left, &left_catalog),
		(right, &right_catalog),
		limits,
		native_profile,
		&mut cancelled,
	)?;
	let policy = Policy::for_options(options, native_profile);
	let topology = Topology::compare(&left_catalog, &right_catalog, &pairs, &mut cancelled)?;
	let mut payloads = Payloads::new(limits.max_bytes, cancelled);
	#[cfg(feature = "content-hash")]
	payloads.set_contents(options.contents);
	let mut xattrs = policy.xattrs.map(|_| Xattrs::default());
	let mut changes = Vec::new();
	let mut equivalent_path_spellings = Vec::new();

	for (left_path, left_entry) in &left_catalog {
		payloads.poll()?;
		let Some((right_path, right_entry)) = pairs
			.right(left_path)
			.and_then(|path| right_catalog.get_key_value(path))
		else {
			changes.push(Change::new(left_path, ChangeKind::Removed, Vec::new()));
			continue;
		};
		let paths = PairedPaths {
			left: left_path,
			right: right_path,
		};
		let mut entry = compare_entry(
			(&mut *left, left_entry),
			(&mut *right, right_entry),
			paths,
			&topology,
			&mut payloads,
			#[cfg(feature = "finder-layout")]
			policy.finder_layout,
		)?;
		let xattr_changes = if let Some(xattrs) = &mut xattrs {
			xattrs.compare(
				(left, &left_entry.node),
				(right, &right_entry.node),
				paths,
				&mut payloads,
			)?
		} else {
			Vec::new()
		};
		if !xattr_changes.is_empty() {
			entry.fields.push(Field::ExtendedAttributes);
		}
		if options.finder_info
			&& left_entry.metadata.kind == right_entry.metadata.kind
			&& !finder_info::equivalent(
				(left, &left_entry.node),
				(right, &right_entry.node),
				paths,
				&mut payloads,
			)? {
			entry.fields.push(Field::FinderInfo);
		}
		if !entry.fields.is_empty() {
			let mut change = Change::new(left_path, ChangeKind::Changed, entry.fields);
			if left_path != right_path {
				change.right_path = Some(right_path.clone());
			}
			change.xattrs = xattr_changes;
			#[cfg(feature = "finder-layout")]
			{
				change.finder_layout = entry.finder_layout;
			}
			changes.push(change);
		} else if left_path != right_path {
			equivalent_path_spellings.push(EquivalentPathSpelling {
				left: left_path.clone(),
				right: right_path.clone(),
			});
		}
	}
	for right_path in right_catalog.keys() {
		payloads.poll()?;
		if pairs
			.left(right_path)
			.is_none_or(|path| !left_catalog.contains_key(path))
		{
			changes.push(Change::new(right_path, ChangeKind::Added, Vec::new()));
		}
	}
	changes.sort_unstable_by(|left, right| left.path.cmp(&right.path));
	#[cfg(feature = "content-hash")]
	let rename_candidates = if options.infer_renames {
		renames::infer(
			(left, &left_catalog),
			(right, &right_catalog),
			&pairs,
			&mut payloads,
		)?
	} else {
		Vec::new()
	};
	payloads.poll()?;

	Ok(Report {
		schema_version: SCHEMA_VERSION,
		outcome: if changes.is_empty() {
			Outcome::Equal
		} else {
			Outcome::Different
		},
		policy,
		limits,
		left_entries: left_catalog.len(),
		right_entries: right_catalog.len(),
		compared_bytes: payloads.compared_bytes(),
		changes,
		equivalent_path_spellings,
		#[cfg(feature = "content-hash")]
		rename_candidates,
	})
}

fn require_kind<L: StdError + 'static, R: StdError + 'static>(
	entry: &Entry<impl Copy>,
	side: Side,
	path: &[String],
) -> Result<(), Error<L, R>> {
	match entry.metadata.kind {
		NodeKind::Directory | NodeKind::RegularFile | NodeKind::Symlink => Ok(()),
		_ => Err(Error::UnsupportedKind {
			side,
			path: path.to_vec(),
		}),
	}
}
