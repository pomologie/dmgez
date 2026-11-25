//! Filesystem comparisons with independently admitted disk windows.

use std::{error::Error as StdError, hash::Hash, num::NonZeroU32, path::Path};

#[cfg(feature = "apfs-volume")]
use dmgez_apfs_read::{AdapterError, SourceError, TransactionId, VolumeInfo, VolumeSelector};
use dmgez_diff::{
	Side,
	semantic::{self, Limits, Options},
};
#[cfg(feature = "hfsplus-volume")]
use dmgez_hfsplus_read::browse::{Error as HfsError, VolumeSignature};
use dmgez_inspector::SelectedWindowId;
use dmgez_vfs::{
	LogicalDisk, VolumeId,
	volume::{VolumeBackend, VolumeView},
};
use thiserror::Error;

use super::{InputError, require_regular_file};
use crate::{
	disk::{self, Encoding},
	volume::{self, Error as VolumeError},
};

/// Regular-file input and explicit disk window for a semantic comparison.
#[derive(Debug, Clone, Copy)]
pub struct Input<'a> {
	pub path: &'a Path,
	pub encoding: Encoding,
	/// Optional one-based partition slot. Otherwise requires a unique window.
	pub partition: Option<NonZeroU32>,
}

/// APFS input with optional volume and checkpoint selectors.
#[cfg(feature = "apfs-volume")]
#[derive(Debug, Clone, Copy)]
pub struct ApfsInput<'a> {
	pub disk: Input<'a>,
	pub volume: Option<VolumeSelector>,
	pub checkpoint: Option<TransactionId>,
}

/// A successful comparison and the exact windows and volumes compared.
#[derive(Debug)]
#[non_exhaustive]
pub struct Report<I> {
	pub left: ComparedInput<I>,
	pub right: ComparedInput<I>,
	pub comparison: semantic::Report,
}

/// Identity of a filesystem volume examined within one image.
#[derive(Debug)]
#[non_exhaustive]
pub struct ComparedInput<I> {
	pub window: SelectedWindowId,
	pub identity: I,
}

/// Failure admitting the requested filesystem inputs or comparing their contents.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error<E: StdError + 'static> {
	#[error(transparent)]
	Input(#[from] InputError<VolumeError>),
	#[error("unsupported selected disk window for the {0} input")]
	UnsupportedWindow(Side),
	#[error(transparent)]
	Compare(#[from] semantic::Error<E, E>),
}

/// Compares two admitted HFS+ or HFSX views.
///
/// # Errors
/// Returns a side-specific input error or a failed comparison.
#[cfg(feature = "hfsplus-volume")]
pub fn hfsplus(
	left: Input<'_>,
	right: Input<'_>,
	limits: Limits,
	options: Options,
	cancelled: impl FnMut() -> bool,
) -> Result<Report<VolumeSignature>, Error<HfsError>> {
	check_paths(left, right)?;
	let opened_left = with_input(left, Side::Left, |disk| {
		volume::with_hfs_disk(disk, left.partition, |left_view, left_signature| {
			with_input(right, Side::Right, |disk| {
				volume::with_hfs_disk(disk, right.partition, |right_view, right_signature| {
					compare_open(
						left_view,
						right_view,
						left_signature,
						right_signature,
						limits,
						options,
						cancelled,
					)
				})
			})
		})
	})?;
	opened_left?
}

/// Compares two admitted APFS views under the requested checkpoints.
///
/// # Errors
/// Returns a side-specific input error or a failed comparison. Checkpoint
/// recovery diagnostics require explicit transaction selection on that input.
#[cfg(feature = "apfs-volume")]
pub fn apfs(
	left: ApfsInput<'_>,
	right: ApfsInput<'_>,
	limits: Limits,
	options: Options,
	cancelled: impl FnMut() -> bool,
) -> Result<Report<VolumeInfo>, Error<AdapterError<SourceError>>> {
	check_paths(left.disk, right.disk)?;
	let opened_left = with_apfs_input(left, Side::Left, |left_view, left_info| {
		with_apfs_input(right, Side::Right, |right_view, right_info| {
			compare_open(
				left_view, right_view, left_info, right_info, limits, options, cancelled,
			)
		})
	})?;
	opened_left?
}

#[cfg(feature = "apfs-volume")]
fn with_apfs_input<T>(
	input: ApfsInput<'_>,
	side: Side,
	read: impl FnOnce(&mut volume::ApfsView<'_, '_, '_>, VolumeInfo) -> T,
) -> Result<T, InputError<VolumeError>> {
	with_input(input.disk, side, |disk| {
		volume::with_apfs_disk(
			disk,
			input.disk.partition,
			input.checkpoint,
			|selected, container| {
				volume::require_apfs_checkpoint(input.checkpoint, container.diagnostics())?;
				Ok(volume::with_apfs_volume(
					selected,
					container,
					input.volume,
					read,
				)?)
			},
		)?
	})
}

fn check_paths(left: Input<'_>, right: Input<'_>) -> Result<(), InputError<VolumeError>> {
	for (side, input) in [(Side::Left, left), (Side::Right, right)] {
		require_regular_file(input.path).map_err(|source| InputError::new(side, source.into()))?;
	}
	Ok(())
}

fn with_input<T>(
	input: Input<'_>,
	side: Side,
	read: impl FnOnce(&mut dyn LogicalDisk) -> Result<T, VolumeError>,
) -> Result<T, InputError<VolumeError>> {
	// Keep later comparison failures in T so this source is still revalidated.
	disk::with_path(input.path, input.encoding, read)
		.map_err(|source| InputError::new(side, source))
}

fn selected<I, E: StdError + 'static>(
	window: VolumeId,
	identity: I,
	side: Side,
) -> Result<ComparedInput<I>, Error<E>> {
	Ok(ComparedInput {
		window: SelectedWindowId::from_volume(window).ok_or(Error::UnsupportedWindow(side))?,
		identity,
	})
}

fn compare_open<L, R, I, E>(
	left: &mut VolumeView<L>,
	right: &mut VolumeView<R>,
	left_identity: I,
	right_identity: I,
	limits: Limits,
	options: Options,
	cancelled: impl FnMut() -> bool,
) -> Result<Report<I>, Error<E>>
where
	E: StdError + 'static,
	L: VolumeBackend<Error = E>,
	R: VolumeBackend<Error = E>,
	L::Id: Eq + Hash,
	R::Id: Eq + Hash,
{
	let left_input = selected(left.partition(), left_identity, Side::Left)?;
	let right_input = selected(right.partition(), right_identity, Side::Right)?;
	let comparison = semantic::compare_with_options(left, right, limits, options, cancelled)?;
	Ok(Report {
		left: left_input,
		right: right_input,
		comparison,
	})
}
