use std::io::{self, Read, Write};

use crate::{Error, SECTOR_BYTES, blkx::Kind, trailer};

use super::{DataFork, OutputMetadata, Profile, StoredChunk, WriteOptions, write_image};

#[derive(Default)]
struct ByteCounter(u64);

impl Write for ByteCounter {
	fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
		self.0 = self.0.checked_add(bytes.len() as u64).unwrap();
		Ok(bytes.len())
	}

	fn flush(&mut self) -> io::Result<()> {
		Ok(())
	}
}

struct UnreadSource;

impl Read for UnreadSource {
	fn read(&mut self, _output: &mut [u8]) -> io::Result<usize> {
		panic!("geometry rejection must precede source reads")
	}
}

#[test]
fn block_map_limit_admits_the_last_chunk_and_refuses_an_extra_sector() {
	let options = WriteOptions {
		profile: Profile::Udro,
		..WriteOptions::default()
	};
	let maximum = WriteOptions::MAX_DATA_RUNS as u64 * options.chunk_bytes as u64;
	assert_eq!(
		options.max_data_runs(maximum).unwrap(),
		WriteOptions::MAX_DATA_RUNS
	);
	let mut sink = ByteCounter::default();
	assert!(matches!(
		write_image(UnreadSource, maximum + SECTOR_BYTES, &mut sink, &options),
		Err(Error::ResourceLimit("maximum block-map records"))
	));
	assert_eq!(sink.0, 0);

	// Position the writer at the final chunk without streaming the prefix.
	let length = options.chunk_bytes as u64;
	let prefix = maximum - length;
	let mut data = DataFork::new(&mut sink, 1, options.max_physical_bytes);
	data.stored_bytes = prefix;
	data.sectors = prefix / SECTOR_BYTES;
	data.write(StoredChunk {
		sectors: length / SECTOR_BYTES,
		kind: Kind::Raw,
		bytes: vec![0; options.chunk_bytes],
	})
	.unwrap();
	assert_eq!(data.records[0].offset, prefix);
	assert_eq!(data.stored_bytes, maximum);
	assert_eq!(data.sectors * SECTOR_BYTES, maximum);
	assert_eq!(data.sink.0, length);
}

#[test]
fn physical_cursor_admits_the_last_sector_and_refuses_wrap_before_writing() {
	let ceiling = u64::MAX - trailer::BYTES as u64;
	let mut sink = ByteCounter::default();
	let mut data = DataFork::new(&mut sink, 1, u64::MAX);
	data.stored_bytes = ceiling - SECTOR_BYTES;
	let sector = || StoredChunk {
		sectors: 1,
		kind: Kind::Raw,
		bytes: vec![0; SECTOR_BYTES as usize],
	};

	data.write(sector()).unwrap();
	assert_eq!(data.stored_bytes, ceiling);
	assert_eq!(data.records[0].offset, ceiling - SECTOR_BYTES);
	assert!(matches!(
		data.write(sector()),
		Err(Error::ResourceLimit("physical data fork size"))
	));
	// Seed the arithmetic boundary without streaming an unrepresentable prefix.
	data.stored_bytes = u64::MAX - SECTOR_BYTES + 1;
	assert!(matches!(
		data.write(sector()),
		Err(Error::ResourceLimit("physical data fork size"))
	));
	assert_eq!(data.sink.0, SECTOR_BYTES);
}

#[test]
fn metadata_cursor_overflow_leaves_the_sink_untouched() {
	let options = WriteOptions {
		profile: Profile::Udro,
		max_physical_bytes: u64::MAX,
		..WriteOptions::default()
	};
	let mut sink = ByteCounter::default();
	let mut data = DataFork::new(&mut sink, 1, options.max_physical_bytes);
	// The trailer alone fits, but the block-map plist plus trailer cannot.
	data.stored_bytes = u64::MAX - trailer::BYTES as u64;
	data.sectors = 1;
	assert!(matches!(
		data.finish(0, &options, 0, OutputMetadata::default()),
		Err(Error::ResourceLimit("physical image size"))
	));
	assert_eq!(sink.0, 0);
}
