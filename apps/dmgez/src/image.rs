use std::{fs::File, sync::Arc};

use dmgez_inspector::{
	DiskLimits, InspectionReport, RunPage, RunWindow, VerificationMode, VerificationReport,
	inspect_disk, inspect_runs, verify_with_cancellation,
};
use dmgez_udif::{
	Cancellation, Error,
	reader::{FileSource, Image, Limits, ReadAt},
};

pub(crate) const RUN_PAGE_SIZE: usize = 64;

pub(crate) struct ImageTab {
	pub snapshot: Arc<ImageSnapshot>,
	pub report: InspectionReport,
	pub verification: Option<Box<VerificationReport>>,
	#[cfg(feature = "hfsplus-read")]
	pub browser: browse::Browser,
}

#[cfg(feature = "hfsplus-read")]
pub(crate) mod browse;

impl ImageTab {
	pub(crate) fn open(file: File) -> Result<Self, Error> {
		let source = ImageSource(Arc::new(FileSource::new(file)?));
		let mut image = Image::open_release(source.clone(), Limits::default())?;
		let report = inspect_disk(
			&mut image,
			RunWindow::new(0, RUN_PAGE_SIZE),
			DiskLimits::default(),
		);
		source.check_unchanged()?;
		Ok(Self {
			snapshot: Arc::new(ImageSnapshot { source, image }),
			report,
			verification: None,
			#[cfg(feature = "hfsplus-read")]
			browser: browse::Browser::default(),
		})
	}
}

pub(crate) struct ImageSnapshot {
	source: ImageSource,
	image: Image<ImageSource>,
}

impl ImageSnapshot {
	pub(crate) fn run_page(&self, offset: usize) -> Result<RunPage, Error> {
		let window = RunWindow::new(offset, RUN_PAGE_SIZE)
			.ok_or(Error::ResourceLimit("desktop run page size"))?;
		let page = inspect_runs(&self.image, window);
		self.source.check_unchanged()?;
		Ok(page)
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
