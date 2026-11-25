use dmgez_udif::writer::WriteReport;

/// Geometry of a completed output and its encoding-specific evidence.
#[derive(Debug)]
#[non_exhaustive]
pub struct ImageReport {
	pub logical_bytes: u64,
	pub physical_bytes: u64,
	pub encoding: ImageEncoding,
}

/// Metadata present in the selected output encoding.
#[derive(Debug)]
#[non_exhaustive]
pub enum ImageEncoding {
	/// A UDIF image with declared checksums and an encoded run table.
	Udif(WriteReport),
	/// A trailerless raw-backed UDRW image without container checksums.
	Udrw,
}

impl ImageReport {
	pub(super) fn udif(report: WriteReport) -> Self {
		Self {
			logical_bytes: report.logical_bytes,
			physical_bytes: report.physical_bytes,
			encoding: ImageEncoding::Udif(report),
		}
	}

	pub(super) fn udrw(logical_bytes: u64) -> Self {
		Self {
			logical_bytes,
			physical_bytes: logical_bytes,
			encoding: ImageEncoding::Udrw,
		}
	}
}
