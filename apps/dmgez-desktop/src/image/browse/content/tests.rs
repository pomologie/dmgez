use super::{Extent, MAX_TEXT_BYTES, Preview, classify};

#[test]
fn separates_valid_text_from_control_bytes_and_unsupported_encoding() {
	let text = "Résumé\t✓\r\n";
	assert_eq!(
		classify(text.as_bytes().to_vec(), text.len() as u64),
		Preview::Text {
			value: text.into(),
			extent: Extent::Complete,
		}
	);
	assert_eq!(classify(b"text\0".to_vec(), 5), Preview::Binary);
	assert_eq!(classify(vec![0xff], 1), Preview::UnsupportedEncoding);
	assert_eq!(classify(vec![0xe2], 1), Preview::UnsupportedEncoding);
	assert_eq!(classify(vec![0xc2, 0x9b], 2), Preview::Binary);
}

#[test]
fn a_truncated_multibyte_character_preserves_the_valid_sample_prefix() {
	let mut sample = vec![b'a'; MAX_TEXT_BYTES - 1];
	sample.push(0xe2);
	let file_bytes = (MAX_TEXT_BYTES + 2) as u64;
	let expected = Preview::Text {
		value: "a".repeat(MAX_TEXT_BYTES - 1),
		extent: Extent::Prefix { file_bytes },
	};
	assert_eq!(classify(sample, file_bytes), expected);
}
