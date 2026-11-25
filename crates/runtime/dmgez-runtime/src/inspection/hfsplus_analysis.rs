use std::{
	num::{NonZeroU32, NonZeroUsize},
	path::Path,
};

use dmgez_hfsplus::analysis::{self, MAX_LARGEST_FILES};
use dmgez_inspector::{AnalysisReport, SelectedWindowId};
use dmgez_udif::reader::{Image, Limits as ImageLimits};
use dmgez_vfs::{Disk, Limits as DiskLimits};
use thiserror::Error;

use crate::{
	disk::{SelectionError, select_volume},
	image::open_file,
};

/// Container and bounded filesystem analysis for one HFS+ or HFSX window.
#[derive(Debug)]
#[non_exhaustive]
pub struct HfsAnalysis {
	pub container: AnalysisReport,
	pub volume: SelectedWindowId,
	pub filesystem: analysis::Report,
}

/// Analysis could not admit the image or complete the requested checks.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum HfsAnalysisError {
	#[error("ranked HFS-family file limit must be from 1 to {MAX_LARGEST_FILES}")]
	LargestFiles,
	#[error("unsupported HFS+ analysis window")]
	UnsupportedWindow,
	#[error(transparent)]
	Image(#[from] dmgez_udif::Error),
	#[error(transparent)]
	Disk(#[from] dmgez_vfs::Error),
	#[error(transparent)]
	Selection(#[from] SelectionError),
	#[error(transparent)]
	Analysis(#[from] analysis::Error),
}

/// Analyzes allocation and ranks a bounded number of files in a selected HFS+ or HFSX volume.
///
/// # Errors
/// Returns typed configuration, admission, filesystem or source revalidation errors.
pub fn analyze_hfsplus(
	path: &Path,
	partition: Option<NonZeroU32>,
	largest: Option<NonZeroUsize>,
) -> Result<HfsAnalysis, HfsAnalysisError> {
	let mut limits = analysis::Limits::default();
	if let Some(largest) = largest {
		let largest = largest.get();
		if largest > MAX_LARGEST_FILES {
			return Err(HfsAnalysisError::LargestFiles);
		}
		limits.largest_files = largest;
	}

	let mut image = Image::open_release(open_file(path)?, ImageLimits::default())?;
	let container = dmgez_inspector::analyze(&image);
	let mut disk = Disk::discover(&mut image, DiskLimits::default())?;
	let selected = select_volume(&disk, partition)?;
	let volume =
		SelectedWindowId::from_volume(selected).ok_or(HfsAnalysisError::UnsupportedWindow)?;
	let filesystem = analysis::analyze(&mut disk, selected, limits)?;
	image.check_source_unchanged()?;

	Ok(HfsAnalysis {
		container,
		volume,
		filesystem,
	})
}
