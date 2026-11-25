use std::{fs::File, path::PathBuf, sync::Arc};

use dmgez_inspector::{
	DiskLimits, InspectionReport, RunPage, RunWindow, VerificationMode, VerificationReport,
	verify_with_cancellation,
};
use dmgez_runtime::image::{
	ImageSession, InspectionScope, MetadataBudget, RunLocation, RunQuery, RunQueryPage,
};
use dmgez_udif::{
	Cancellation, Error,
	reader::{FileSource, Limits, ReadAt},
};

pub(crate) const RUN_PAGE_SIZE: usize = 64;

pub(crate) struct ImageTab {
	pub snapshot: Arc<ImageSnapshot>,
	pub report: InspectionReport,
	pub verification: Option<Arc<VerificationReport>>,
	pub exported: Option<PathBuf>,
	pub run_navigation: RunNavigation,
	#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
	pub browser: browse::Browser,
}

#[derive(Default)]
pub(crate) struct RunNavigation {
	pub offset: String,
	pub invalid_offset: bool,
	pub selected: Option<usize>,
	pub reveal_selected: bool,
	pub query: RunQuery,
	pub query_page: Option<RunQueryPage>,
}

impl RunNavigation {
	pub(crate) fn select(&mut self, ordinal: Option<usize>) {
		self.query = RunQuery::default();
		self.query_page = None;
		self.select_displayed(ordinal);
	}

	pub(crate) fn select_displayed(&mut self, ordinal: Option<usize>) {
		self.selected = ordinal;
		self.reveal_selected = ordinal.is_some();
	}
}

#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
pub(crate) mod browse;
pub(crate) mod compare;
pub(crate) mod exports;

impl ImageTab {
	pub(crate) fn open(file: File) -> Result<Self, Error> {
		let source = ImageSource(Arc::new(FileSource::new(file)?));
		let mut image = ImageSession::open(
			source.clone(),
			Limits::default(),
			MetadataBudget::ReaderLimits,
		)?;
		let report = image.inspect(
			InspectionScope::Disk(DiskLimits::default()),
			RunWindow::new(0, RUN_PAGE_SIZE),
		)?;
		Ok(Self {
			snapshot: Arc::new(ImageSnapshot { source, image }),
			report,
			verification: None,
			exported: None,
			run_navigation: RunNavigation::default(),
			#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
			browser: browse::Browser::default(),
		})
	}
}

pub(crate) struct ImageSnapshot {
	source: ImageSource,
	image: ImageSession<ImageSource>,
}

impl ImageSnapshot {
	pub(crate) fn run_page(&self, offset: usize) -> Result<RunPage, Error> {
		let window = RunWindow::new(offset, RUN_PAGE_SIZE)
			.ok_or(Error::ResourceLimit("desktop run page size"))?;
		self.image.run_page(window)
	}

	pub(crate) fn locate_run(&self, offset: u64) -> Result<RunLocation, Error> {
		self.image.locate_run(offset, RUN_PAGE_SIZE)
	}

	pub(crate) fn query_runs(
		&self,
		query: RunQuery,
		offset: usize,
		cancellation: &Cancellation,
	) -> Result<RunQueryPage, Error> {
		self.image
			.query_runs(query, offset, RUN_PAGE_SIZE, cancellation)
	}

	pub(crate) fn verify(&self, cancellation: &Cancellation) -> VerificationReport {
		verify_with_cancellation(
			self.source.clone(),
			Limits::default(),
			VerificationMode::Container,
			cancellation,
		)
	}
}

#[derive(Clone)]
struct ImageSource(Arc<FileSource>);

impl ReadAt for ImageSource {
	fn len(&self) -> u64 {
		self.0.len()
	}

	fn check_unchanged(&self) -> Result<(), Error> {
		self.0.check_unchanged()
	}

	fn read_exact_at(&self, offset: u64, output: &mut [u8]) -> Result<(), Error> {
		self.0.read_exact_at(offset, output)
	}
}

#[cfg(test)]
mod tests;
