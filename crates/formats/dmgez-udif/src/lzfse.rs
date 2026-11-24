#[cfg(feature = "read")]
pub(crate) fn decode(mut stored: &[u8], length: usize) -> Option<Vec<u8>> {
	let physical_bytes = stored.len() as u64;
	let mut decoded = vec![0; length];
	// Keep writes inside the mapping, including for a block whose declared raw size is hostile.
	let (consumed, produced) = lzfse_rust::LzfseRingDecoder::default()
		.decode(&mut stored, &mut decoded.as_mut_slice())
		.ok()?;
	if consumed != physical_bytes || produced != length as u64 || !stored.is_empty() {
		return None;
	}
	Some(decoded)
}

#[cfg(feature = "write")]
pub(crate) fn encode(mut raw: &[u8]) -> std::io::Result<Option<Vec<u8>>> {
	use std::io::ErrorKind;

	// If the encoded stream would be as large as the input, the caller emits a raw run.
	let ceiling = raw.len();
	let mut output = vec![0; ceiling];
	let result =
		lzfse_rust::LzfseRingEncoder::default().encode(&mut raw, &mut output.as_mut_slice());
	match result {
		Ok((consumed, _)) if consumed != ceiling as u64 || !raw.is_empty() => {
			Err(std::io::Error::new(
				ErrorKind::InvalidData,
				"LZFSE encoder did not consume the chunk",
			))
		}
		Ok((_, produced)) if produced < ceiling as u64 => {
			output.truncate(produced as usize);
			Ok(Some(output))
		}
		Ok(_) => Ok(None),
		Err(error) if error.kind() == ErrorKind::WriteZero => Ok(None),
		Err(error) => Err(error),
	}
}

#[cfg(all(test, feature = "read"))]
mod tests {
	use super::decode;

	const RAW_BLOCK: &[u8] = b"bvx-\x04\x00\x00\x00testbvx$";
	const RAW_LENGTH: usize = b"test".len();

	#[test]
	fn rejects_truncation_trailing_data_and_wrong_decoded_size() {
		assert_eq!(
			decode(RAW_BLOCK, RAW_LENGTH).as_deref(),
			Some(b"test".as_slice())
		);
		assert!(decode(RAW_BLOCK, RAW_LENGTH - 1).is_none());
		assert!(decode(RAW_BLOCK, RAW_LENGTH + 1).is_none());
		for len in 0..RAW_BLOCK.len() {
			assert!(decode(&RAW_BLOCK[..len], RAW_LENGTH).is_none());
		}
		let concatenated = [RAW_BLOCK, RAW_BLOCK].concat();
		assert!(decode(&concatenated, RAW_LENGTH).is_none());
		assert!(decode(&concatenated, 2 * RAW_LENGTH).is_none());
	}
}
