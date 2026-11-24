#[cfg(feature = "read")]
use lzma_rust2::{Action, Status, XzStream};

#[cfg(feature = "read")]
const MAX_DECODE_WORKSPACE_KIB: u32 = 9 << 10;

#[cfg(feature = "read")]
pub(crate) fn decode(stored: &[u8], length: usize) -> Option<Vec<u8>> {
	frame::admit(stored, length)?;
	// A spare byte lets the decoder reach the XZ footer when data ends exactly at the bound.
	let mut decoded = vec![0; length.checked_add(1)?];
	let mut decoder = XzStream::new_mem_limit(false, MAX_DECODE_WORKSPACE_KIB);
	let result = decoder.process(stored, &mut decoded, Action::Finish).ok()?;
	if result.status != Status::StreamEnd
		|| result.bytes_consumed != stored.len()
		|| result.bytes_produced != length
	{
		return None;
	}
	decoded.truncate(length);
	Some(decoded)
}

#[cfg(feature = "write")]
const ENCODER_PRESET: u32 = 1;

#[cfg(feature = "write")]
pub(crate) fn encode(raw: &[u8]) -> std::io::Result<Option<Vec<u8>>> {
	use std::io::{self, ErrorKind, Write};

	use lzma_rust2::{CheckType, XzOptions, XzWriter};

	let mut output = vec![0; raw.len()];
	let result: io::Result<usize> = (|| {
		let mut options = XzOptions::with_preset(ENCODER_PRESET);
		options.set_check_sum_type(CheckType::None);
		let mut encoder = XzWriter::new(output.as_mut_slice(), options)?;
		encoder.write_all(raw)?;
		Ok(encoder.finish()?.len())
	})();
	match result {
		Ok(0) => Ok(None),
		Ok(remaining) => {
			output.truncate(raw.len() - remaining);
			Ok(Some(output))
		}
		Err(error) if error.kind() == ErrorKind::WriteZero => Ok(None),
		Err(error) => Err(error),
	}
}

#[cfg(feature = "read")]
mod frame {
	// ULMO uses an XZ stream with one LZMA2 block and no internal payload check.
	const SIGNATURE_AND_FLAGS: &[u8] = b"\xfd7zXZ\0\0\0";
	const STREAM_HEADER_BYTES: usize = 12;
	const STREAM_FOOTER_BYTES: usize = 12;
	const ALIGNMENT: usize = 4;
	const CRC_BYTES: usize = 4;
	const INDEX_LENGTH_FIELD: std::ops::Range<usize> = 4..8;
	const ONE_BLOCK_INDEX: &[u8] = &[0, 1];
	const SIZE_FLAGS: u8 = HAS_COMPRESSED_SIZE | HAS_UNCOMPRESSED_SIZE;
	const HAS_COMPRESSED_SIZE: u8 = 0x40;
	const HAS_UNCOMPRESSED_SIZE: u8 = 0x80;
	const LZMA2_FILTER: &[u8] = &[0x21, 1];
	const MAX_DICTIONARY_PROPERTY: u8 = 22; // 8 MiB
	const VLI_CONTINUATION: u8 = 0x80;
	const VLI_VALUE: u8 = 0x7f;
	const VLI_BITS_PER_BYTE: usize = 7;
	const VLI_BITS: usize = 63;

	pub(super) fn admit(stored: &[u8], length: usize) -> Option<()> {
		if !stored.starts_with(SIGNATURE_AND_FLAGS) {
			return None;
		}
		let footer = stored.last_chunk::<STREAM_FOOTER_BYTES>()?;
		let index_size = u32::from_le_bytes(footer[INDEX_LENGTH_FIELD].try_into().ok()?) as usize;
		let index_size = index_size.checked_add(1)?.checked_mul(ALIGNMENT)?;
		let index_end = stored.len() - STREAM_FOOTER_BYTES;
		let index_start = index_end.checked_sub(index_size)?;
		let mut index = stored
			.get(index_start..index_end)?
			.strip_prefix(ONE_BLOCK_INDEX)?;
		let unpadded = usize::try_from(vli(&mut index)?).ok()?;
		let decoded = usize::try_from(vli(&mut index)?).ok()?;
		let padded = unpadded.checked_next_multiple_of(ALIGNMENT)?;
		let block = stored.get(STREAM_HEADER_BYTES..index_start)?;
		if decoded != length || padded != block.len() {
			return None;
		}
		check_filter(block, unpadded, length)
	}

	fn check_filter(block: &[u8], unpadded: usize, length: usize) -> Option<()> {
		let (&size, content) = block.split_first()?;
		if size == 0 {
			return None;
		}
		let header_len = (usize::from(size) + 1) * ALIGNMENT;
		let compressed_len = unpadded.checked_sub(header_len)?;
		let header = content.get(..header_len - 1 - CRC_BYTES)?;
		let (&flags, mut fields) = header.split_first()?;
		if flags & !SIZE_FLAGS != 0 {
			return None;
		}
		if flags & HAS_COMPRESSED_SIZE != 0 && vli(&mut fields)? != compressed_len as u64 {
			return None;
		}
		if flags & HAS_UNCOMPRESSED_SIZE != 0 && vli(&mut fields)? != length as u64 {
			return None;
		}
		let (&property, padding) = fields.strip_prefix(LZMA2_FILTER)?.split_first()?;
		(property <= MAX_DICTIONARY_PROPERTY && padding.iter().all(|&byte| byte == 0)).then_some(())
	}

	fn vli(input: &mut &[u8]) -> Option<u64> {
		let mut value = 0;
		for shift in (0..VLI_BITS).step_by(VLI_BITS_PER_BYTE) {
			let (&byte, rest) = input.split_first()?;
			*input = rest;
			value |= u64::from(byte & VLI_VALUE) << shift;
			if byte & VLI_CONTINUATION == 0 {
				return (shift == 0 || byte != 0).then_some(value);
			}
		}
		None
	}
}

#[cfg(all(test, feature = "read", feature = "write"))]
mod tests {
	use super::{decode, encode};

	const DECODED_BYTES: usize = 512;
	const DATA: [u8; DECODED_BYTES] = [b'A'; DECODED_BYTES];

	#[test]
	fn rejects_partial_concatenated_and_wrong_length_streams() {
		let stored = encode(&DATA).unwrap().unwrap();
		assert_eq!(
			decode(&stored, DATA.len()).as_deref(),
			Some(DATA.as_slice())
		);
		assert!(decode(&stored, DATA.len() - 1).is_none());
		assert!(decode(&stored, DATA.len() + 1).is_none());
		assert!(decode(&[stored.as_slice(), &stored].concat(), DATA.len()).is_none());
		for prefix in 0..stored.len() {
			assert!(decode(&stored[..prefix], DATA.len()).is_none());
		}
	}

	#[test]
	fn rejects_larger_dictionary_before_allocating_it() {
		const BLOCK_HEADER_START: usize = 12;
		const DICTIONARY_PROPERTY_FROM_HEADER: usize = 4;
		const DICTIONARY_16_MIB: u8 = 24;
		const CRC_BYTES: usize = 4;
		let mut stored = encode(&DATA).unwrap().unwrap();
		let header_end = BLOCK_HEADER_START + (usize::from(stored[BLOCK_HEADER_START]) + 1) * 4;
		let crc_start = header_end - CRC_BYTES;
		stored[BLOCK_HEADER_START + DICTIONARY_PROPERTY_FROM_HEADER] = DICTIONARY_16_MIB;
		let hash = crc32fast::hash(&stored[BLOCK_HEADER_START..crc_start]);
		stored[crc_start..header_end].copy_from_slice(&hash.to_le_bytes());
		assert!(decode(&stored, DATA.len()).is_none());
	}
}
