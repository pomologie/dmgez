use std::{
	collections::{BTreeMap, btree_map::Entry},
	error::Error as StdError,
};

use crate::Side;
use dmgez_vfs::volume::{
	LookupName, NativeNameKey, NativeNameProfile, ViewError, VolumeBackend, VolumeView,
};

use super::{Error, Limits, PathMatching, catalog::Catalog};

const ROOT_PATH_BYTES: usize = 1;
const SEPARATOR_BYTES: usize = 1;

/// Native path equivalence could not be established for the selected views.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PathMatchError<L: StdError + 'static, R: StdError + 'static> {
	#[error("the {side} filesystem native path profile is unavailable")]
	UnavailableProfile { side: Side },
	#[error("native filesystem path profiles differ (left {left:?}, right {right:?})")]
	IncompatibleProfiles {
		left: NativeNameProfile,
		right: NativeNameProfile,
	},
	#[error("the {side} filesystem has ambiguous native paths {first:?} and {second:?}")]
	Ambiguous {
		side: Side,
		first: Vec<String>,
		second: Vec<String>,
	},
	#[error("the {side} filesystem native paths exceed the {limit} aggregate key bytes limit")]
	KeyBytesLimit { side: Side, limit: usize },
	#[error("the {side} filesystem native path key is unavailable for {path:?}")]
	UnavailableKey { side: Side, path: Vec<String> },
	#[error("could not obtain a native name key for the left path {path:?}")]
	LeftName {
		path: Vec<String>,
		#[source]
		source: ViewError<L>,
	},
	#[error("could not obtain a native name key for the right path {path:?}")]
	RightName {
		path: Vec<String>,
		#[source]
		source: ViewError<R>,
	},
}

type StoredPath<'catalog> = &'catalog [String];
type NativeIndex<'catalog> = BTreeMap<Vec<NativeNameKey>, StoredPath<'catalog>>;
type Counterparts<'catalog> = BTreeMap<StoredPath<'catalog>, StoredPath<'catalog>>;

pub(super) enum Pairs<'catalog> {
	Exact,
	Native {
		left_to_right: Counterparts<'catalog>,
		right_to_left: Counterparts<'catalog>,
	},
}

pub(super) fn native_profile<L: VolumeBackend, R: VolumeBackend>(
	left: &VolumeView<L>,
	right: &VolumeView<R>,
	mode: PathMatching,
	cancelled: &mut impl FnMut() -> bool,
) -> Result<Option<NativeNameProfile>, Error<L::Error, R::Error>> {
	match mode {
		PathMatching::ExactStoredText => return Ok(None),
		PathMatching::Native => {}
	}
	check_cancelled(cancelled)?;
	let left_profile = left
		.native_name_profile()
		.ok_or(PathMatchError::UnavailableProfile { side: Side::Left })?;
	let right_profile = right
		.native_name_profile()
		.ok_or(PathMatchError::UnavailableProfile { side: Side::Right })?;
	if left_profile != right_profile {
		return Err(PathMatchError::IncompatibleProfiles {
			left: left_profile,
			right: right_profile,
		}
		.into());
	}
	Ok(Some(left_profile))
}

impl<'catalog> Pairs<'catalog> {
	pub(super) fn new<L: VolumeBackend, R: VolumeBackend>(
		(left, left_catalog): (&VolumeView<L>, &'catalog Catalog<L::Id>),
		(right, right_catalog): (&VolumeView<R>, &'catalog Catalog<R::Id>),
		limits: Limits,
		profile: Option<NativeNameProfile>,
		cancelled: &mut impl FnMut() -> bool,
	) -> Result<Self, Error<L::Error, R::Error>> {
		if profile.is_none() {
			return Ok(Self::Exact);
		}
		let left = native_index(
			left,
			left_catalog,
			limits.max_path_bytes,
			Side::Left,
			cancelled,
			|path, source| PathMatchError::LeftName {
				path: path.to_vec(),
				source,
			},
		)?;
		let right = native_index(
			right,
			right_catalog,
			limits.max_path_bytes,
			Side::Right,
			cancelled,
			|path, source| PathMatchError::RightName {
				path: path.to_vec(),
				source,
			},
		)?;
		let mut left_to_right = BTreeMap::new();
		let mut right_to_left = BTreeMap::new();
		for (key, left) in left {
			check_cancelled(cancelled)?;
			if let Some(&right) = right.get(&key) {
				left_to_right.insert(left, right);
				right_to_left.insert(right, left);
			}
		}
		Ok(Self::Native {
			left_to_right,
			right_to_left,
		})
	}

	/// Candidate stored path on the right. Exact matching reuses the left
	/// path spelling and leaves presence checking to the right catalog.
	pub(super) fn right(&self, path: StoredPath<'catalog>) -> Option<StoredPath<'catalog>> {
		match self {
			Self::Exact => Some(path),
			Self::Native { left_to_right, .. } => left_to_right.get(path).copied(),
		}
	}

	/// Candidate stored path on the left. Exact matching reuses the right
	/// path spelling and leaves presence checking to the left catalog.
	pub(super) fn left(&self, path: StoredPath<'catalog>) -> Option<StoredPath<'catalog>> {
		match self {
			Self::Exact => Some(path),
			Self::Native { right_to_left, .. } => right_to_left.get(path).copied(),
		}
	}
}

fn native_index<'catalog, B: VolumeBackend, L: StdError + 'static, R: StdError + 'static>(
	view: &VolumeView<B>,
	catalog: &'catalog Catalog<B::Id>,
	limit: usize,
	side: Side,
	cancelled: &mut impl FnMut() -> bool,
	invalid_name: impl Fn(&[String], ViewError<B::Error>) -> PathMatchError<L, R>,
) -> Result<NativeIndex<'catalog>, Error<L, R>> {
	let mut index = BTreeMap::new();
	let mut remaining = limit;
	let limit_error = || PathMatchError::KeyBytesLimit { side, limit };
	for path in catalog.keys() {
		check_cancelled(cancelled)?;
		remaining = remaining
			.checked_sub(ROOT_PATH_BYTES)
			.ok_or_else(limit_error)?;
		let mut key = Vec::with_capacity(path.len());
		for (position, name) in path.iter().enumerate() {
			check_cancelled(cancelled)?;
			let component = view
				.native_name_key(LookupName::Utf8(name))
				.map_err(|source| invalid_name(path, source))?
				.ok_or_else(|| PathMatchError::UnavailableKey {
					side,
					path: path.clone(),
				})?;
			let separator = if position == 0 { 0 } else { SEPARATOR_BYTES };
			remaining = remaining
				.checked_sub(component.as_bytes().len())
				.and_then(|available| available.checked_sub(separator))
				.ok_or_else(limit_error)?;
			key.push(component);
		}
		match index.entry(key) {
			Entry::Vacant(entry) => {
				entry.insert(path.as_slice());
			}
			Entry::Occupied(entry) => {
				return Err(PathMatchError::Ambiguous {
					side,
					first: entry.get().to_vec(),
					second: path.clone(),
				}
				.into());
			}
		}
	}
	Ok(index)
}

fn check_cancelled<L: StdError + 'static, R: StdError + 'static>(
	cancelled: &mut impl FnMut() -> bool,
) -> Result<(), Error<L, R>> {
	if cancelled() {
		return Err(Error::Cancelled);
	}
	Ok(())
}
