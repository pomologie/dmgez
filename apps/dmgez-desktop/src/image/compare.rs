#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
use std::num::NonZeroU32;
use std::path::Path;

#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
use dmgez_diff::semantic::{self, Options};
use dmgez_diff::{DEFAULT_COMPARE_BYTES, Report as ByteReport};
use dmgez_runtime::compare;
#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
use dmgez_runtime::{compare::semantic as compare_filesystem, disk::Encoding};
use dmgez_udif::{Cancellation, reader::Limits};
use thiserror::Error;

use super::ImageSnapshot;

#[derive(Clone, Copy, Debug)]
pub(crate) enum Request {
	Bytes(compare::ByteSpace),
	#[cfg(feature = "hfsplus-read")]
	HfsPlus {
		left: Selection,
		right: Selection,
		options: Options,
	},
	#[cfg(feature = "apfs-read")]
	Apfs {
		left: Selection,
		right: Selection,
		options: Options,
	},
}

#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Selection {
	pub partition: Option<NonZeroU32>,
	#[cfg(feature = "apfs-read")]
	pub volume: Option<dmgez_apfs::ObjectId>,
	#[cfg(feature = "apfs-read")]
	pub checkpoint: Option<dmgez_apfs::TransactionId>,
}

#[derive(Debug)]
pub(crate) enum Report {
	Bytes {
		address_space: compare::ByteSpace,
		report: ByteReport,
	},
	#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
	Filesystem(Box<FilesystemReport>),
}

#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
#[derive(Debug)]
pub(crate) struct FilesystemReport {
	pub left: String,
	pub right: String,
	pub comparison: semantic::Report,
}

#[derive(Debug, Error)]
pub(crate) enum Error {
	#[error("could not open the comparison image")]
	Open(#[source] dmgez_udif::Error),
	#[error(transparent)]
	Bytes(#[from] compare::ByteError),
	#[cfg(feature = "hfsplus-read")]
	#[error(transparent)]
	HfsPlus(#[from] compare_filesystem::Error<dmgez_hfsplus::browse::Error>),
	#[cfg(feature = "apfs-read")]
	#[error(transparent)]
	Apfs(#[from] compare_filesystem::Error<dmgez_apfs::AdapterError<dmgez_apfs::SourceError>>),
}

impl ImageSnapshot {
	pub(crate) fn compare_file(
		&self,
		path: &Path,
		request: Request,
		cancellation: &Cancellation,
	) -> Result<Report, Error> {
		let left = self.source.clone();
		let right = compare::capture_file(path).map_err(Error::Open)?;
		let cancelled = || cancellation.is_cancelled();
		match request {
			Request::Bytes(address_space) => {
				let report =
					compare::sources(left, right, address_space, DEFAULT_COMPARE_BYTES, cancelled)?;
				Ok(Report::Bytes {
					address_space,
					report,
				})
			}
			#[cfg(feature = "hfsplus-read")]
			Request::HfsPlus {
				left: left_selection,
				right: right_selection,
				options,
			} => Ok(hfsplus_report(compare_filesystem::hfsplus_sources(
				left_selection.input(left),
				right_selection.input(right),
				semantic::Limits::default(),
				options,
				cancelled,
			)?)),
			#[cfg(feature = "apfs-read")]
			Request::Apfs {
				left: left_selection,
				right: right_selection,
				options,
			} => Ok(apfs_report(compare_filesystem::apfs_sources(
				left_selection.apfs(left),
				right_selection.apfs(right),
				semantic::Limits::default(),
				options,
				cancelled,
			)?)),
		}
	}
}

#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
impl Selection {
	fn input<S>(self, source: S) -> compare_filesystem::DiskInput<S> {
		compare_filesystem::DiskInput {
			source,
			encoding: Encoding::Udif(Limits::default()),
			partition: self.partition,
		}
	}

	#[cfg(feature = "apfs-read")]
	fn apfs<S>(self, source: S) -> compare_filesystem::ApfsDiskInput<S> {
		compare_filesystem::ApfsDiskInput {
			disk: self.input(source),
			volume: self.volume.map(dmgez_apfs::VolumeSelector::ObjectId),
			checkpoint: self.checkpoint,
		}
	}
}

impl Request {
	pub(crate) fn logical() -> Self {
		Self::Bytes(compare::ByteSpace::Logical(Limits::default()))
	}
}

#[cfg(feature = "hfsplus-read")]
fn hfsplus_report(
	report: compare_filesystem::Report<dmgez_hfsplus::browse::VolumeSignature>,
) -> Report {
	use dmgez_hfsplus::browse::VolumeSignature;

	fn label(volume: &compare_filesystem::ComparedInput<VolumeSignature>) -> String {
		let filesystem = match volume.identity {
			VolumeSignature::HfsPlus => "HFS+",
			VolumeSignature::HfsX => "HFSX",
		};
		format!("{filesystem} · {}", window_label(volume.window))
	}

	Report::Filesystem(Box::new(FilesystemReport {
		left: label(&report.left),
		right: label(&report.right),
		comparison: report.comparison,
	}))
}

#[cfg(feature = "apfs-read")]
fn apfs_report(report: compare_filesystem::Report<dmgez_apfs::VolumeInfo>) -> Report {
	fn label(volume: &compare_filesystem::ComparedInput<dmgez_apfs::VolumeInfo>) -> String {
		let name = volume.identity.name.as_str().escape_debug();
		let object = volume.identity.object_id.0;
		let checkpoint = volume.identity.checkpoint.0;
		let window = window_label(volume.window);
		format!("APFS · {name} · {window} · object {object} · XID {checkpoint}")
	}

	Report::Filesystem(Box::new(FilesystemReport {
		left: label(&report.left),
		right: label(&report.right),
		comparison: report.comparison,
	}))
}

#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
fn window_label(window: dmgez_inspector::SelectedWindowId) -> String {
	use dmgez_inspector::SelectedWindowId;

	match window {
		SelectedWindowId::WholeDisk => "Whole disk".into(),
		SelectedWindowId::Partition { id } => format!("Partition {}", id.slot),
		_ => "Selected disk window".into(),
	}
}
