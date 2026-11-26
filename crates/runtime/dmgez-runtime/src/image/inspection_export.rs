use std::{io, path::Path};

use dmgez_core::output::{PublishError, publish_new};
use dmgez_inspector::{InspectionReport, VerificationReport};
use dmgez_udif::{
	Cancellation, Error as ImageError,
	reader::{FileSource, Image, Limits, ReadAt},
};
use thiserror::Error;

use crate::disk::BorrowedSource;

use super::ImageSession;

const COPY_BUFFER_BYTES: usize = 64 << 10;

/// Maximum logical length of one exported run, in bytes.
pub const MAX_RUN_EXPORT_BYTES: u64 = 64 << 20;

/// A completed report produced for the retained image session.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub enum ReportExport<'report> {
	/// The retained inspection report and its current run page.
	Inspection(&'report InspectionReport),
	/// The result of a completed verification operation.
	Verification(&'report VerificationReport),
}

/// Failure while reading the source or writing staged export data.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ExportDataError {
	#[error("image export cancelled")]
	Cancelled,
	#[error(transparent)]
	Image(#[from] ImageError),
	#[error("could not serialize image report")]
	Json(#[from] serde_json::Error),
	#[error("could not write export data")]
	Io(#[from] io::Error),
}

/// Why an inspector report or logical run could not be exported.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum InspectionExportError {
	#[error(transparent)]
	Data(#[from] ExportDataError),
	#[error("the selected logical run is outside the image map")]
	RunNotFound,
	#[error("export requires a decoder for the selected logical run")]
	RunDecoderUnavailable,
	#[error(
		"the selected logical run has {bytes} bytes and exceeds the {MAX_RUN_EXPORT_BYTES}-byte export limit"
	)]
	RunTooLarge { bytes: u64 },
	#[error(transparent)]
	Publication(#[from] PublishError<ExportDataError>),
}

impl InspectionExportError {
	/// Whether the retained image changed since admission.
	#[must_use]
	pub fn source_changed(&self) -> bool {
		matches!(
			self.data_error(),
			Some(ExportDataError::Image(ImageError::SourceChanged))
		)
	}

	/// Whether the export stopped before publication due to cancellation.
	#[must_use]
	pub fn cancelled(&self) -> bool {
		matches!(self.data_error(), Some(ExportDataError::Cancelled))
	}

	fn data_error(&self) -> Option<&ExportDataError> {
		match self {
			Self::Data(error)
			| Self::Publication(PublishError::Write(error) | PublishError::Validate(error)) => Some(error),
			_ => None,
		}
	}
}

impl<S: ReadAt> ImageSession<S> {
	/// Save a report derived from this held image as versioned JSON at a new path.
	///
	/// # Errors
	/// Returns a source, cancellation, serialization, or publication error.
	/// An existing destination is preserved.
	pub fn export_json(
		&self,
		report: ReportExport<'_>,
		destination: &Path,
		cancellation: &Cancellation,
	) -> Result<(), InspectionExportError> {
		let check = || -> Result<(), ExportDataError> {
			check_cancellation(cancellation)?;
			Ok(self.image.check_source_unchanged()?)
		};
		check()?;
		publish_new(
			destination,
			|stage| -> Result<(), ExportDataError> {
				match report {
					ReportExport::Inspection(report) => {
						serde_json::to_writer_pretty(&mut *stage, report)?
					}
					ReportExport::Verification(report) => {
						serde_json::to_writer_pretty(&mut *stage, report)?
					}
				}
				stage.write_all(b"\n")?;
				Ok(())
			},
			|_, ()| check(),
		)?;
		Ok(())
	}
}

/// Stream the bytes of one zero-based logical run to a new path.
///
/// Pass the held source used to produce the selected run index. The output is
/// raw logical disk data without a container, signature, or verification report.
/// Ignore and zero runs produce logical zero bytes.
///
/// # Errors
/// Returns an error for invalid or unsupported runs, runs over the export limit,
/// changed sources, cancelled copies or failed publication. An existing
/// destination is preserved.
pub fn export_run(
	source: &FileSource,
	ordinal: usize,
	destination: &Path,
	cancellation: &Cancellation,
) -> Result<(), InspectionExportError> {
	check_cancellation(cancellation)?;
	source.check_unchanged().map_err(ExportDataError::from)?;
	let image = Image::open_release(BorrowedSource(source), Limits::default())
		.map_err(ExportDataError::from)?;
	let run = image
		.runs()
		.nth(ordinal)
		.ok_or(InspectionExportError::RunNotFound)?;
	let bytes = run.logical.end - run.logical.start;
	if bytes > MAX_RUN_EXPORT_BYTES {
		return Err(InspectionExportError::RunTooLarge { bytes });
	}
	if !run.encoding.can_decode() {
		return Err(InspectionExportError::RunDecoderUnavailable);
	}
	publish_new(
		destination,
		|stage| -> Result<(), ExportDataError> {
			let mut buffer = vec![0; COPY_BUFFER_BYTES];
			let mut offset = run.logical.start;
			while offset < run.logical.end {
				check_cancellation(cancellation)?;
				let count = (run.logical.end - offset).min(COPY_BUFFER_BYTES as u64) as usize;
				image.read_exact_at(offset, &mut buffer[..count])?;
				stage.write_all(&buffer[..count])?;
				offset += count as u64;
			}
			Ok(())
		},
		|_, ()| {
			check_cancellation(cancellation)?;
			Ok(source.check_unchanged()?)
		},
	)?;
	Ok(())
}

fn check_cancellation(cancellation: &Cancellation) -> Result<(), ExportDataError> {
	if cancellation.is_cancelled() {
		Err(ExportDataError::Cancelled)
	} else {
		Ok(())
	}
}
