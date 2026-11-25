//! Exact comparison of visible paths and supported objects in two volume views.

use std::{error::Error as StdError, hash::Hash};

use dmgez_vfs::volume::{ForkKind, NodeKind, ViewError, VolumeBackend, VolumeView};
use thiserror::Error;

use crate::{SCHEMA_VERSION, Side};

mod catalog;
mod links;
mod payloads;
mod report;

#[cfg(test)]
mod tests;

pub use catalog::CatalogError;
pub use report::{
	Change, ChangeKind, ContentComparison, Field, Limits, Outcome, PathMatching, Policy, Report,
	Uncompared,
};

use catalog::{Entry, collect};
use links::Topology;
use payloads::Payloads;

/// A stopped comparison does not produce a supported-scope equality result.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error<L: StdError + 'static, R: StdError + 'static> {
	#[error("could not collect the left filesystem catalog")]
	LeftCatalog(#[source] CatalogError<L>),
	#[error("could not collect the right filesystem catalog")]
	RightCatalog(#[source] CatalogError<R>),
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
	mut cancelled: impl FnMut() -> bool,
) -> Result<Report, Error<L::Error, R::Error>>
where
	L: VolumeBackend,
	R: VolumeBackend,
	L::Id: Eq + Hash,
	R::Id: Eq + Hash,
{
	let left_catalog = collect(left, limits, &mut cancelled).map_err(Error::LeftCatalog)?;
	let right_catalog = collect(right, limits, &mut cancelled).map_err(Error::RightCatalog)?;
	let topology = Topology::compare(&left_catalog, &right_catalog, &mut cancelled)?;
	let mut payloads = Payloads::new(limits.max_bytes, cancelled);
	let mut changes = Vec::new();

	for (path, left_entry) in &left_catalog {
		payloads.poll()?;
		let Some(right_entry) = right_catalog.get(path) else {
			changes.push(Change::new(path, ChangeKind::Removed, Vec::new()));
			continue;
		};
		let fields = compare_entry(
			(left, left_entry),
			(right, right_entry),
			path,
			&topology,
			&mut payloads,
		)?;
		if !fields.is_empty() {
			changes.push(Change::new(path, ChangeKind::Changed, fields));
		}
	}
	for path in right_catalog.keys() {
		payloads.poll()?;
		if !left_catalog.contains_key(path) {
			changes.push(Change::new(path, ChangeKind::Added, Vec::new()));
		}
	}
	changes.sort_unstable_by(|left, right| left.path.cmp(&right.path));
	payloads.poll()?;

	Ok(Report {
		schema_version: SCHEMA_VERSION,
		outcome: if changes.is_empty() {
			Outcome::Equal
		} else {
			Outcome::Different
		},
		policy: Policy::default(),
		limits,
		left_entries: left_catalog.len(),
		right_entries: right_catalog.len(),
		compared_bytes: payloads.compared_bytes(),
		changes,
	})
}

fn compare_entry<L: VolumeBackend, R: VolumeBackend>(
	left: (&mut VolumeView<L>, &Entry<L::Id>),
	right: (&mut VolumeView<R>, &Entry<R::Id>),
	path: &[String],
	topology: &Topology<'_>,
	payloads: &mut Payloads<impl FnMut() -> bool>,
) -> Result<Vec<Field>, Error<L::Error, R::Error>> {
	let (left_view, left) = left;
	let (right_view, right) = right;
	let l = &left.metadata;
	let r = &right.metadata;
	let mut fields = Vec::new();
	for (same, field) in [
		(l.kind == r.kind, Field::Kind),
		(l.mode == r.mode, Field::Mode),
		(l.uid == r.uid, Field::Uid),
		(l.gid == r.gid, Field::Gid),
		(topology.equivalent(l, r), Field::HardLinkTopology),
	] {
		if !same {
			fields.push(field);
		}
	}
	match (l.kind, r.kind) {
		(NodeKind::RegularFile, NodeKind::RegularFile) => {
			for (kind, field) in [
				(ForkKind::Data, Field::DataFork),
				(ForkKind::Resource, Field::ResourceFork),
			] {
				if !payloads.fork(
					(&mut *left_view, &left.node),
					(&mut *right_view, &right.node),
					kind,
					path,
				)? {
					fields.push(field);
				}
			}
		}
		(NodeKind::Symlink, NodeKind::Symlink)
			if !payloads.link((left_view, &left.node), (right_view, &right.node), path)? =>
		{
			fields.push(Field::SymlinkTarget);
		}
		_ => {}
	}
	Ok(fields)
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
