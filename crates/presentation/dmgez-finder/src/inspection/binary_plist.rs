use super::{ReadBudget, ReadError, ReadLimits, leaf::Cursor};

pub(super) const PLIST_MAGIC: &[u8; 8] = b"bplist00";
const TRAILER_BYTES: usize = 32;
pub(super) const TRAILER_PADDING_BYTES: usize = 6;
const INTEGER_WIDTHS: [u8; 5] = [1, 2, 3, 4, 8];
pub(super) const MARKER_KIND: u8 = 0xf0;
pub(super) const MARKER_VALUE: u8 = 0x0f;
pub(super) const INTEGER_MARKER: u8 = 0x10;
pub(super) const ARRAY_MARKER: u8 = 0xa0;
pub(super) const ORDERED_SET_MARKER: u8 = 0xb0;
pub(super) const SET_MARKER: u8 = 0xc0;
pub(super) const DICTIONARY_MARKER: u8 = 0xd0;
pub(super) const DICTIONARY_REFS_PER_ENTRY: usize = 2;
const EXTENDED_LENGTH: u8 = MARKER_VALUE;
const MAX_LENGTH_WIDTH_POWER: u8 = 3;

pub(super) struct OffsetTable<'a> {
	pub root: usize,
	pub reference_width: usize,
	pub objects: &'a [u8],
	pub has_padding: bool,
	addresses: &'a [u8],
	offset_width: usize,
}

impl<'a> OffsetTable<'a> {
	pub(super) fn read(blob: &'a [u8], limits: ReadLimits) -> Result<Self, ReadError> {
		let trailer_start = blob
			.len()
			.checked_sub(TRAILER_BYTES)
			.ok_or(ReadError::PlistLayout)?;
		let (body, trailer) = blob.split_at(trailer_start);
		if !body.starts_with(PLIST_MAGIC) {
			return Err(ReadError::PlistProfile);
		}
		let mut trailer = Cursor::new(trailer);
		trailer.take(TRAILER_PADDING_BYTES)?;
		let [offset_width, reference_width] = trailer.array()?;
		if !INTEGER_WIDTHS.contains(&offset_width) || !INTEGER_WIDTHS.contains(&reference_width) {
			return Err(ReadError::PlistLayout);
		}
		let count = integer(trailer.take(size_of::<u64>())?)?;
		let root = integer(trailer.take(size_of::<u64>())?)?;
		let table_offset = integer(trailer.take(size_of::<u64>())?)?;
		if count > limits.plist_objects {
			return Err(ReadError::Limit(ReadBudget::PlistObjects));
		}
		if root >= count || !(PLIST_MAGIC.len()..=body.len()).contains(&table_offset) {
			return Err(ReadError::PlistLayout);
		}
		let (objects, offsets) = body.split_at(table_offset);
		let offset_width = usize::from(offset_width);
		let table_bytes = count
			.checked_mul(offset_width)
			.ok_or(ReadError::PlistLayout)?;
		let addresses = offsets.get(..table_bytes).ok_or(ReadError::PlistLayout)?;
		Ok(Self {
			root,
			reference_width: usize::from(reference_width),
			objects,
			has_padding: addresses.len() != offsets.len(),
			addresses,
			offset_width,
		})
	}

	pub(super) fn addresses(&self) -> impl ExactSizeIterator<Item = Result<usize, ReadError>> {
		self.addresses.chunks_exact(self.offset_width).map(integer)
	}
}

pub(super) fn object_length(marker: u8, input: &mut Cursor<'_>) -> Result<usize, ReadError> {
	let compact = marker & MARKER_VALUE;
	if compact != EXTENDED_LENGTH {
		return Ok(usize::from(compact));
	}
	let [marker] = input.array()?;
	let power = marker & MARKER_VALUE;
	if marker & MARKER_KIND != INTEGER_MARKER || power > MAX_LENGTH_WIDTH_POWER {
		return Err(ReadError::PlistLayout);
	}
	integer(input.take(1 << power)?)
}

pub(super) fn integer(bytes: &[u8]) -> Result<usize, ReadError> {
	let mut value = [0; size_of::<u64>()];
	let start = value
		.len()
		.checked_sub(bytes.len())
		.ok_or(ReadError::PlistLayout)?;
	value[start..].copy_from_slice(bytes);
	u64::from_be_bytes(value)
		.try_into()
		.map_err(|_| ReadError::PlistLayout)
}
