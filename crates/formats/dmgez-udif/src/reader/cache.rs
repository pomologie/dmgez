use std::{collections::VecDeque, io};

#[cfg(all(test, dmgez_loom))]
use loom::sync::Mutex;
#[cfg(not(all(test, dmgez_loom)))]
use std::sync::Mutex;

use crate::{Error, geometry::DECODER_SPARE_BYTES};

const MAX_ENTRIES: usize = 2;

#[derive(Debug)]
pub(super) struct DecodeCache {
	entries: Mutex<VecDeque<Entry>>,
	byte_limit: usize,
}

#[derive(Debug)]
struct Entry {
	run: usize,
	bytes: Vec<u8>,
}

impl DecodeCache {
	pub(super) fn new(max_decoded_bytes: usize) -> Self {
		Self {
			entries: Mutex::new(VecDeque::new()),
			byte_limit: max_decoded_bytes + DECODER_SPARE_BYTES,
		}
	}

	pub(super) fn clear(&mut self) -> Result<(), Error> {
		self.entries.get_mut().map_err(|_| poisoned())?.clear();
		Ok(())
	}

	pub(super) fn copy(
		&self,
		run: usize,
		reserved: usize,
		within: usize,
		output: &mut [u8],
		decode: impl FnOnce() -> Result<Vec<u8>, Error>,
	) -> Result<(), Error> {
		// Keep ownership through decode and copy to prevent duplicate concurrent
		// work and keep the combined decoded allocation within the byte limit.
		let mut entries = self.entries.lock().map_err(|_| poisoned())?;
		let entry = match entries.iter().position(|entry| entry.run == run) {
			Some(position) => entries
				.remove(position)
				.ok_or(Error::Corrupt("missing decoded cache entry"))?,
			None => {
				let available = self
					.byte_limit
					.checked_sub(reserved)
					.ok_or(Error::ResourceLimit("decoded cache bytes"))?;
				while entries.len() == MAX_ENTRIES
					|| entries
						.iter()
						.map(|entry| entry.bytes.capacity())
						.sum::<usize>() > available
				{
					entries.pop_front();
				}
				let bytes = decode()?;
				if bytes.capacity() > reserved {
					return Err(Error::ResourceLimit("decoded cache bytes"));
				}
				Entry { run, bytes }
			}
		};
		let end = within
			.checked_add(output.len())
			.ok_or(Error::Corrupt("decoded run range overflow"))?;
		let bytes = entry
			.bytes
			.get(within..end)
			.ok_or(Error::Corrupt("read outside decoded run"))?;
		output.copy_from_slice(bytes);
		entries.push_back(entry);
		Ok(())
	}
}

fn poisoned() -> Error {
	io::Error::other("UDIF decode cache lock poisoned").into()
}
