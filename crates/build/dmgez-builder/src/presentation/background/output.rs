use std::io::{self, Cursor, Seek, SeekFrom, Write};

use tiff::{
	TiffError, TiffResult,
	encoder::{DirectoryEncoder, Rational, TiffKindStandard},
	tags::{ResolutionUnit, Tag},
};

use super::{Error, RETINA_SCALE, TIFF_TOP_LEFT};

pub(super) const BASE_DPI: u32 = 72;
pub(super) const RETINA_DPI: u32 = BASE_DPI * RETINA_SCALE;
const EXACT_RESOLUTION_DENOMINATOR: u32 = 1;

pub(super) struct CappedOutput {
	buffer: Cursor<Vec<u8>>,
	limit: usize,
}

impl CappedOutput {
	pub(super) const fn new(limit: usize) -> Self {
		Self {
			buffer: Cursor::new(Vec::new()),
			limit,
		}
	}

	pub(super) fn into_bytes(self) -> Vec<u8> {
		self.buffer.into_inner()
	}
}

impl Write for CappedOutput {
	fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
		let end = usize::try_from(self.buffer.position())
			.ok()
			.and_then(|position| position.checked_add(bytes.len()))
			.filter(|&end| end <= self.limit)
			.ok_or(io::ErrorKind::FileTooLarge)?;
		let data = self.buffer.get_mut();
		if end > data.capacity() {
			let target = end.max(data.capacity().saturating_mul(2)).min(self.limit);
			data.try_reserve_exact(target - data.len())
				.map_err(io::Error::other)?;
			if data.capacity() > self.limit {
				return Err(io::ErrorKind::FileTooLarge.into());
			}
		}
		self.buffer.write(bytes)
	}

	fn flush(&mut self) -> io::Result<()> {
		Ok(())
	}
}

impl Seek for CappedOutput {
	fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
		self.buffer.seek(position)
	}
}

pub(super) fn write_tiff_resolution<W: Write + Seek>(
	tags: &mut DirectoryEncoder<'_, W, TiffKindStandard>,
	dpi: u32,
) -> TiffResult<()> {
	let resolution = Rational {
		n: dpi,
		d: EXACT_RESOLUTION_DENOMINATOR,
	};
	tags.write_tag(Tag::ResolutionUnit, ResolutionUnit::Inch)?;
	tags.write_tag(Tag::XResolution, resolution.clone())?;
	tags.write_tag(Tag::YResolution, resolution)?;
	tags.write_tag(Tag::Orientation, TIFF_TOP_LEFT)
}

pub(super) fn tiff_encode_error(error: TiffError) -> Error {
	match error {
		TiffError::IoError(source) if source.kind() == io::ErrorKind::FileTooLarge => {
			Error::MetadataBudget
		}
		other => Error::BackgroundTiff(other),
	}
}
