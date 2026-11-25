//! Retained, read-only sessions for inspection of a single image source.

use std::{fs::File, path::Path};

use dmgez_inspector::{
	AnalysisReport, DiskLimits, InspectionReport, RunPage, RunWindow, analyze, inspect,
	inspect_disk, inspect_runs,
};
use dmgez_udif::{
	Error,
	reader::{FileSource, Image, Limits, ReadAt},
};

#[cfg(feature = "raw-export")]
mod export;
#[cfg(feature = "report-export")]
mod inspection_export;
#[cfg(test)]
mod tests;
#[cfg(feature = "raw-export")]
pub use export::{ExportError, export_raw};
#[cfg(feature = "report-export")]
pub use inspection_export::{
	ExportDataError, InspectionExportError, MAX_RUN_EXPORT_BYTES, ReportExport, export_run,
};

/// How much of the logical disk to inspect.
#[derive(Debug, Clone, Copy)]
pub enum InspectionScope {
	/// Inspect stored container metadata without probing the logical disk.
	Container,
	/// Inspect the container and identify supported partitions and volumes.
	Disk(DiskLimits),
}

/// How to bound initial reads of trailer, XML and signature metadata.
#[derive(Debug, Clone, Copy)]
pub enum MetadataBudget {
	/// Apply the separate limits in the reader configuration.
	ReaderLimits,
	/// Also cap the total admission metadata bytes for this session.
	CombinedBytes(u64),
}

/// A structurally admitted image with a retained index and backing source.
#[derive(Debug)]
pub struct ImageSession<S> {
	image: Image<S>,
}

/// A requested run and its page in the logical disk.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RunLocation {
	/// Bounded page containing the selected run.
	pub page: RunPage,
	/// Zero-based ordinal of the selected run in logical order.
	pub ordinal: usize,
}

/// Opens a regular file for held, read-only image access.
///
/// # Errors
/// Returns an error if the file cannot be opened or admitted as an image source.
pub fn open_file(path: &Path) -> Result<FileSource, Error> {
	FileSource::new(File::open(path)?)
}

impl<S: ReadAt> ImageSession<S> {
	/// Reads and retains the qualified signed or unsigned container metadata.
	///
	/// # Errors
	/// Returns source, format or budget errors during admission.
	pub fn open(source: S, limits: Limits, budget: MetadataBudget) -> Result<Self, Error> {
		let image = match budget {
			MetadataBudget::ReaderLimits => Image::open_release(source, limits),
			MetadataBudget::CombinedBytes(bytes) => {
				Image::open_release_with_metadata_limit(source, limits, bytes)
			}
		}?;
		Ok(Self { image })
	}

	/// Reports image metadata and checks the held source before returning it.
	///
	/// # Errors
	/// Returns an error if the source can no longer be revalidated.
	pub fn inspect(
		&mut self,
		scope: InspectionScope,
		window: Option<RunWindow>,
	) -> Result<InspectionReport, Error> {
		let report = match scope {
			InspectionScope::Container => inspect(&self.image, window),
			InspectionScope::Disk(limits) => inspect_disk(&mut self.image, window, limits),
		};
		self.image.check_source_unchanged()?;
		Ok(report)
	}

	/// Reports a page from the retained run index after revalidating its source.
	///
	/// # Errors
	/// Returns a source change or I/O error before delivering the page.
	pub fn run_page(&self, window: RunWindow) -> Result<RunPage, Error> {
		let page = inspect_runs(&self.image, window);
		self.image.check_source_unchanged()?;
		Ok(page)
	}

	/// Reports the run covering a disk-relative byte and the page containing it.
	///
	/// Lookup and paging use the retained index without decoding payloads.
	///
	/// # Errors
	/// Returns an error for an invalid page size, an out-of-range byte, or an
	/// index whose backing source cannot be revalidated.
	pub fn locate_run(&self, offset: u64, page_size: usize) -> Result<RunLocation, Error> {
		let ordinal = self.image.run_ordinal_at(offset).ok_or(Error::OutOfRange {
			offset,
			len: 1,
			disk_len: self.image.descriptor().logical_bytes,
		})?;
		let window = RunWindow::containing(ordinal, page_size)
			.ok_or(Error::ResourceLimit("run page size"))?;
		Ok(RunLocation {
			page: self.run_page(window)?,
			ordinal,
		})
	}

	/// Reports container and run allocation without decoding payloads.
	///
	/// # Errors
	/// Returns a source change or I/O error before delivering the report.
	pub fn analyze(&self) -> Result<AnalysisReport, Error> {
		let report = analyze(&self.image);
		self.image.check_source_unchanged()?;
		Ok(report)
	}
}
