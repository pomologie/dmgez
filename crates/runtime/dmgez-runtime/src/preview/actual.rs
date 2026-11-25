//! Bounded previews of Finder evidence from actual disk images.

use std::{io, num::NonZeroU32, path::Path};

use dmgez_config::VirtualPath;
use dmgez_core::output::{PublishError, publish_new};
use dmgez_hfsplus::{
	browse::Limits as HfsLimits,
	preview::{CaptureError, capture_disk},
};
use dmgez_preview::{
	ItemVisibility,
	actual::Coverage,
	raster::{self, ActualPreview},
};
use dmgez_udif::reader::ReadAt as _;
use dmgez_vfs::{Disk, Limits as DiskLimits, LogicalDisk, VolumeId};
use thiserror::Error;

use crate::{
	disk::{self, BorrowedSource, Encoding, SelectionError},
	image::open_file,
};

const MAX_FS_READ_REQUESTS: u64 = 131_072;
const MAX_FS_READ_BYTES: u64 = 256 << 20;

/// A selected folder could not be captured or rendered.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RenderError {
	#[error(transparent)]
	Capture(#[from] CaptureError),
	#[error(transparent)]
	Raster(#[from] raster::Error),
}

/// Captures and renders a folder addressed by exact stored UTF-16 names.
///
/// # Errors
/// Returns an error for unreadable or unsupported folder data or exceeded preview bounds.
pub fn render_disk<S: LogicalDisk>(
	disk: &mut Disk<S>,
	volume: VolumeId,
	limits: HfsLimits,
	folder: &[Vec<u16>],
	visibility: ItemVisibility,
) -> Result<ActualPreview, RenderError> {
	Ok(capture_disk(disk, volume, limits, folder)?.render(visibility)?)
}

/// An image preview could not be admitted, rendered, or published.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ImageError {
	#[error(transparent)]
	Image(#[from] dmgez_udif::Error),
	#[error(transparent)]
	Disk(#[from] dmgez_vfs::Error),
	#[error(transparent)]
	Selection(#[from] SelectionError),
	#[error(transparent)]
	Render(#[from] RenderError),
	#[error(transparent)]
	Publish(#[from] PublishError<io::Error>),
}

/// Size and Finder evidence of a successfully published image preview.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct Report {
	/// PNG width and height in pixels.
	pub size: (u32, u32),
	/// Availability of Finder layout and artwork, and resulting position counts.
	pub coverage: Coverage,
}

/// Render an actual HFS+ or HFSX folder to a new PNG.
///
/// The same held source is checked after rendering and again before publication.
///
/// # Errors
/// Returns an error for unreadable or modified images, unavailable or unsupported
/// folders, exceeded preview bounds, or failed publication.
pub fn image_png(
	image: &Path,
	encoding: Encoding,
	partition: Option<NonZeroU32>,
	folder: &VirtualPath,
	visibility: ItemVisibility,
	output: &Path,
) -> Result<Report, ImageError> {
	let components: Vec<_> = folder
		.components()
		.map(|name| name.encode_utf16().collect())
		.collect();
	let source = open_file(image)?;
	let preview = disk::with_logical_disk(BorrowedSource(&source), encoding, |logical| {
		let mut disk = Disk::discover(logical, DiskLimits::default())?;
		let selected = disk::select_volume(&disk, partition)?;
		let limits = HfsLimits {
			max_read_requests: MAX_FS_READ_REQUESTS,
			max_read_bytes: MAX_FS_READ_BYTES,
			..HfsLimits::default()
		};
		Ok::<_, ImageError>(render_disk(
			&mut disk,
			selected,
			limits,
			&components,
			visibility,
		)?)
	})?;
	source.check_unchanged()?;
	publish_new(
		output,
		|output| preview.write_png(output).map_err(io::Error::other),
		|_, ()| source.check_unchanged().map_err(io::Error::other),
	)?;
	Ok(Report {
		size: preview.size(),
		coverage: preview.coverage(),
	})
}
