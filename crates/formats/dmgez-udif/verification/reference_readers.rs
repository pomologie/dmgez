//! Standalone test adapter for the pinned independent UDIF readers.
//! Give it only the synthetic single-table raw or zlib fixtures.

use std::{env, fs, path::PathBuf};

const TRAILER_BYTES: usize = 512;

fn main() -> Result<(), Box<dyn std::error::Error>> {
	let [image, raw]: [PathBuf; 2] = env::args_os()
		.skip(1)
		.map(PathBuf::from)
		.collect::<Vec<_>>()
		.try_into()
		.map_err(|_| "usage: reference-readers IMAGE EXPECTED_RAW")?;
	let expected = fs::read(raw)?;
	let encoded = fs::read(&image)?;
	let mut apple = apple_dmg::DmgReader::open(&image)?;
	let mut trailer = Vec::with_capacity(TRAILER_BYTES);
	apple.koly().write_to(&mut trailer)?;
	assert_eq!(
		trailer.as_slice(),
		encoded
			.last_chunk::<TRAILER_BYTES>()
			.ok_or("missing trailer")?
	);
	assert_eq!(apple.plist().partitions().len(), 1);
	assert_eq!(
		apple.partition_data(0)?,
		expected,
		"apple-dmg decoded bytes"
	);

	let mut udif = udif::DmgArchive::open(&image)?;
	assert_eq!(udif.partitions().len(), 1);
	assert_eq!(
		udif.extract_main_partition()?,
		expected,
		"udif decoded bytes"
	);
	println!("Both reference readers matched {}", image.display());
	Ok(())
}
