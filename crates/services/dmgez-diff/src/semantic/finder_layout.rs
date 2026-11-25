use std::{collections::BTreeMap, error::Error as StdError};

use dmgez_finder::inspection::{
	BackgroundAlias, ReadBudget, ReadError, ReadLimits, RecordValue, Report, inspect,
};
use dmgez_vfs::volume::{Fork, ForkKind, NodeHandle, ViewError, VolumeBackend, VolumeView};

use crate::Side;

use super::{
	ChangeKind, Error, FinderRecordChange,
	object::PairedPaths,
	payloads::{Payloads, missing_data},
};

/// Failure to read the admitted Finder layout from one data fork.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum FinderLayoutError<E: StdError + 'static> {
	#[error("could not read the Finder store data fork")]
	Read(#[source] ViewError<E>),
	#[error(transparent)]
	Inspect(#[from] ReadError),
	#[error("unusable Finder background alias in record {name:?}")]
	BackgroundAlias { name: String },
}

pub(super) fn compare_stores<L: VolumeBackend, R: VolumeBackend>(
	left: (&mut VolumeView<L>, &NodeHandle<L::Id>),
	right: (&mut VolumeView<R>, &NodeHandle<R::Id>),
	paths: PairedPaths<'_>,
	payloads: &mut Payloads<impl FnMut() -> bool>,
) -> Result<Vec<FinderRecordChange>, Error<L::Error, R::Error>> {
	let left_error = |source| Error::LeftFinderLayout {
		path: paths.left.to_vec(),
		source,
	};
	let right_error = |source| Error::RightFinderLayout {
		path: paths.right.to_vec(),
		source,
	};
	payloads.poll()?;
	left.0
		.with_fork(left.1, ForkKind::Data, |left_fork| {
			right
				.0
				.with_fork(right.1, ForkKind::Data, |right_fork| {
					let limits = ReadLimits::default();
					let left_size = store_length(left_fork, limits).map_err(left_error)?;
					let right_size = store_length(right_fork, limits).map_err(right_error)?;
					payloads.charge_bytes(left_fork.len().max(right_fork.len()))?;
					payloads.poll()?;
					let left_store =
						read_store(left_fork, left_size, limits).map_err(left_error)?;
					payloads.poll()?;
					let right_store =
						read_store(right_fork, right_size, limits).map_err(right_error)?;
					Ok(record_changes(left_store, right_store))
				})
				.map_err(|source| right_error(FinderLayoutError::Read(source)))?
				.ok_or_else(|| missing_data(Side::Right, paths.right))?
		})
		.map_err(|source| left_error(FinderLayoutError::Read(source)))?
		.ok_or_else(|| missing_data(Side::Left, paths.left))?
}

fn store_length<E: StdError + 'static>(
	fork: &Fork<'_, E>,
	limits: ReadLimits,
) -> Result<usize, FinderLayoutError<E>> {
	usize::try_from(fork.len())
		.ok()
		.filter(|&len| len <= limits.store_bytes)
		.ok_or_else(|| ReadError::Limit(ReadBudget::StoreBytes).into())
}

fn read_store<E: StdError + 'static>(
	fork: &mut Fork<'_, E>,
	len: usize,
	limits: ReadLimits,
) -> Result<Report, FinderLayoutError<E>> {
	let mut bytes = Vec::new();
	bytes.try_reserve_exact(len).map_err(ReadError::from)?;
	bytes.resize(len, 0);
	fork.read_exact_at(0, &mut bytes)
		.map_err(FinderLayoutError::Read)?;
	let report = inspect(&bytes, limits)?;
	for record in &report.records {
		if let RecordValue::IconView(view) = &record.value
			&& view
				.background_alias
				.as_ref()
				.is_some_and(|alias| !matches!(alias, BackgroundAlias::File(_)))
		{
			return Err(FinderLayoutError::BackgroundAlias {
				name: record.name.clone(),
			});
		}
	}
	Ok(report)
}

fn record_changes(left: Report, right: Report) -> Vec<FinderRecordChange> {
	let mut right = indexed_records(right);
	let mut changes = Vec::new();
	for (identity, left_record) in indexed_records(left) {
		let kind = match right.remove(&identity) {
			Some(right_record) if left_record == right_record => continue,
			Some(_) => ChangeKind::Changed,
			None => ChangeKind::Removed,
		};
		let (name, field) = identity;
		changes.push(FinderRecordChange { name, field, kind });
	}
	changes.extend(right.into_keys().map(|(name, field)| FinderRecordChange {
		name,
		field,
		kind: ChangeKind::Added,
	}));
	changes.sort_unstable_by(|a, b| a.name.cmp(&b.name).then_with(|| a.field.cmp(&b.field)));
	changes
}

type RecordIndex = BTreeMap<(String, [u8; 4]), ([u8; 4], RecordValue)>;

fn indexed_records(report: Report) -> RecordIndex {
	report
		.records
		.into_iter()
		.map(|record| {
			(
				(record.name, record.field),
				(record.data_type, record.value),
			)
		})
		.collect()
}
