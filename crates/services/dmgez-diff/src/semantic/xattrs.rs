use std::{
	collections::{BTreeMap, btree_map::Entry},
	error::Error as StdError,
	num::NonZeroU32,
};

use super::{ChangeKind, Error, XattrChange, XattrPolicy, object::PairedPaths, payloads::Payloads};
use dmgez_vfs::volume::{LookupName, NodeHandle, RawName, ViewError, VolumeBackend, VolumeView};

const RESOURCE_FORK: &str = "com.apple.ResourceFork";
const FINDER_INFO: &str = "com.apple.FinderInfo";
const COMPRESSION_RECORD: &str = "com.apple.decmpfs";
const SYMLINK_RECORD: &str = "com.apple.fs.symlink";
const ACCESS_CONTROL_LIST: &str = "com.apple.system.Security";
const EXCLUDED_NAMES: &[&str] = &[
	RESOURCE_FORK,
	FINDER_INFO,
	COMPRESSION_RECORD,
	SYMLINK_RECORD,
	ACCESS_CONTROL_LIST,
];

pub(super) const POLICY: XattrPolicy = XattrPolicy {
	excluded_names: EXCLUDED_NAMES,
	max_attributes_per_object: NonZeroU32::new(256).unwrap(),
	max_attributes_per_side: 65_536,
	max_name_bytes_per_side: 8 * 1024 * 1024,
};

/// Stopped attribute comparison on one side of a common path.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum XattrError<E: StdError + 'static> {
	#[error("stored attribute enumeration is unavailable")]
	Unavailable,
	#[error("could not enumerate stored attribute names")]
	List(#[source] ViewError<E>),
	#[error("stored attribute name is not valid Unicode text")]
	InvalidName,
	#[error("duplicate stored attribute name {name:?}")]
	DuplicateName { name: String },
	#[error("attribute enumeration exceeded its per-side limit of {limit} names")]
	NameLimit { limit: usize },
	#[error("attribute enumeration exceeded its per-side limit of {limit} name bytes")]
	NameBytesLimit { limit: usize },
	#[error("listed attribute {name:?} is missing")]
	Missing { name: String },
	#[error("could not read stored attribute {name:?}")]
	Read {
		name: String,
		#[source]
		source: ViewError<E>,
	},
}

#[derive(Default)]
pub(super) struct Xattrs {
	left: Budget,
	right: Budget,
}

impl Xattrs {
	pub(super) fn compare<L: VolumeBackend, R: VolumeBackend>(
		&mut self,
		left: (&mut VolumeView<L>, &NodeHandle<L::Id>),
		right: (&mut VolumeView<R>, &NodeHandle<R::Id>),
		paths: PairedPaths<'_>,
		payloads: &mut Payloads<impl FnMut() -> bool>,
	) -> Result<Vec<XattrChange>, Error<L::Error, R::Error>> {
		let (left_view, left_node) = left;
		let (right_view, right_node) = right;
		payloads.poll()?;
		let left_names =
			self.left
				.list(left_view, left_node)
				.map_err(|source| Error::LeftXattrs {
					path: paths.left.to_vec(),
					source,
				})?;
		payloads.poll()?;
		let mut right_names =
			self.right
				.list(right_view, right_node)
				.map_err(|source| Error::RightXattrs {
					path: paths.right.to_vec(),
					source,
				})?;

		let mut changes = Vec::new();
		for (name, left_raw) in left_names {
			payloads.poll()?;
			let kind = if let Some(right_raw) = right_names.remove(&name) {
				if values_equal(
					(left_view, left_node, left_raw.as_lookup()),
					(right_view, right_node, right_raw.as_lookup()),
					paths,
					&name,
					payloads,
				)? {
					continue;
				}
				ChangeKind::Changed
			} else {
				ChangeKind::Removed
			};
			changes.push(XattrChange { name, kind });
		}
		changes.extend(right_names.into_keys().map(|name| XattrChange {
			name,
			kind: ChangeKind::Added,
		}));
		changes.sort_unstable_by(|left, right| left.name.cmp(&right.name));
		Ok(changes)
	}
}

#[derive(Default)]
struct Budget {
	attributes: usize,
	name_bytes: usize,
}

impl Budget {
	fn list<B: VolumeBackend>(
		&mut self,
		view: &mut VolumeView<B>,
		node: &NodeHandle<B::Id>,
	) -> Result<BTreeMap<String, RawName>, XattrError<B::Error>> {
		let names = view
			.list_xattrs(node, POLICY.max_attributes_per_object)
			.map_err(XattrError::List)?
			.ok_or(XattrError::Unavailable)?;
		self.attributes = self
			.attributes
			.checked_add(names.len())
			.filter(|&total| total <= POLICY.max_attributes_per_side)
			.ok_or(XattrError::NameLimit {
				limit: POLICY.max_attributes_per_side,
			})?;

		let mut by_name = BTreeMap::new();
		for raw in names {
			let name = match &raw {
				RawName::Utf8(name) => name.clone(),
				RawName::Utf16(units) => {
					String::from_utf16(units).map_err(|_| XattrError::InvalidName)?
				}
				_ => return Err(XattrError::InvalidName),
			};
			if name.is_empty() || name.contains('\0') {
				return Err(XattrError::InvalidName);
			}
			self.name_bytes = self
				.name_bytes
				.checked_add(name.len())
				.filter(|&total| total <= POLICY.max_name_bytes_per_side)
				.ok_or(XattrError::NameBytesLimit {
					limit: POLICY.max_name_bytes_per_side,
				})?;
			match by_name.entry(name) {
				Entry::Vacant(entry) => entry.insert(raw),
				Entry::Occupied(entry) => {
					return Err(XattrError::DuplicateName {
						name: entry.key().clone(),
					});
				}
			};
		}
		by_name.retain(|name, _| !POLICY.excluded_names.contains(&name.as_str()));
		Ok(by_name)
	}
}

fn values_equal<L: VolumeBackend, R: VolumeBackend>(
	left: (&mut VolumeView<L>, &NodeHandle<L::Id>, LookupName<'_>),
	right: (&mut VolumeView<R>, &NodeHandle<R::Id>, LookupName<'_>),
	paths: PairedPaths<'_>,
	name: &str,
	payloads: &mut Payloads<impl FnMut() -> bool>,
) -> Result<bool, Error<L::Error, R::Error>> {
	let (left_view, left_node, left_name) = left;
	let (right_view, right_node, right_name) = right;
	left_view
		.with_xattr(left_node, left_name, |left_value| {
			right_view
				.with_xattr(right_node, right_name, |right_value| {
					payloads
						.compare_forks(left_value, right_value, paths)
						.map_err(|error| with_attribute_name(error, name))
				})
				.map_err(|source| Error::RightXattrs {
					path: paths.right.to_vec(),
					source: XattrError::Read {
						name: name.to_owned(),
						source,
					},
				})?
				.ok_or_else(|| Error::RightXattrs {
					path: paths.right.to_vec(),
					source: XattrError::Missing {
						name: name.to_owned(),
					},
				})?
		})
		.map_err(|source| Error::LeftXattrs {
			path: paths.left.to_vec(),
			source: XattrError::Read {
				name: name.to_owned(),
				source,
			},
		})?
		.ok_or_else(|| Error::LeftXattrs {
			path: paths.left.to_vec(),
			source: XattrError::Missing {
				name: name.to_owned(),
			},
		})?
}

fn with_attribute_name<L: StdError + 'static, R: StdError + 'static>(
	error: Error<L, R>,
	name: &str,
) -> Error<L, R> {
	match error {
		Error::LeftObject { path, source } => Error::LeftXattrs {
			path,
			source: XattrError::Read {
				name: name.to_owned(),
				source,
			},
		},
		Error::RightObject { path, source } => Error::RightXattrs {
			path,
			source: XattrError::Read {
				name: name.to_owned(),
				source,
			},
		},
		_ => error,
	}
}
