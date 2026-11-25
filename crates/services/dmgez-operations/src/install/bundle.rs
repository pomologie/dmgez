use std::{borrow::Cow, collections::BTreeSet, error::Error as StdError, io::Cursor};

use plist::stream::{Event, Reader};

use super::{Error, check_cancellation};

mod xml;

const BINARY_PLIST_MAGIC: &[u8; 8] = b"bplist00";
const EXECUTABLE_KEY: &str = "CFBundleExecutable";
const MAX_EVENTS: usize = 16_384;
const MAX_DEPTH: usize = 32;
const MAX_EXPANDED_BYTES: u64 = 8 << 20;
const OBJECT_REFERENCE_BYTES: u64 = size_of::<u64>() as u64;
const DICTIONARY_REFERENCES: u64 = 2;

enum Frame {
	Array,
	Dictionary {
		keys: BTreeSet<Cow<'static, str>>,
		awaiting: Option<bool>,
	},
}

impl Frame {
	fn dictionary() -> Self {
		Self::Dictionary {
			keys: BTreeSet::new(),
			awaiting: None,
		}
	}
}

pub(super) fn executable<E: StdError + 'static>(
	bytes: &[u8],
	cancelled: &mut impl FnMut() -> bool,
) -> Result<String, Error<E>> {
	check_cancellation(cancelled)?;
	let plist = if bytes.starts_with(BINARY_PLIST_MAGIC) {
		Cow::Borrowed(bytes)
	} else {
		xml::normalize(bytes, cancelled)?
	};
	let mut reader = Reader::new(Cursor::new(plist.as_ref()));
	let Some(Ok(root @ Event::StartDictionary(_))) = reader.next() else {
		return Err(Error::Info);
	};
	let mut expanded_bytes = expanded_size(&root);
	let mut frames = vec![Frame::dictionary()];
	let mut executable = None;
	for event in reader.by_ref().take(MAX_EVENTS) {
		check_cancellation(cancelled)?;
		let event = event.map_err(|_| Error::Info)?;
		expanded_bytes = expanded_bytes.saturating_add(expanded_size(&event));
		if expanded_bytes > MAX_EXPANDED_BYTES {
			return Err(Error::InfoLimit);
		}
		if matches!(event, Event::EndCollection) {
			if matches!(
				frames.pop(),
				Some(Frame::Dictionary {
					awaiting: Some(_),
					..
				})
			) {
				return Err(Error::Info);
			}
			if frames.is_empty() {
				if reader.next().is_some() {
					return Err(Error::Info);
				}
				return executable.ok_or(Error::Info);
			}
			continue;
		}
		let top_level = frames.len() == 1;
		if let Some(Frame::Dictionary { keys, awaiting }) = frames.last_mut() {
			match awaiting.take() {
				Some(true) => {
					let Event::String(name) = event else {
						return Err(Error::Info);
					};
					executable = Some(name.into_owned());
					continue;
				}
				Some(false) => {}
				None => {
					let Event::String(key) = event else {
						return Err(Error::Info);
					};
					*awaiting = Some(top_level && key == EXECUTABLE_KEY);
					if !keys.insert(key) {
						return Err(Error::Info);
					}
					continue;
				}
			}
		}
		let frame = match event {
			Event::StartArray(_) => Frame::Array,
			Event::StartDictionary(_) => Frame::dictionary(),
			_ => continue,
		};
		if frames.len() >= MAX_DEPTH {
			return Err(Error::InfoLimit);
		}
		frames.push(frame);
	}
	Err(Error::Info)
}

fn expanded_size(event: &Event<'_>) -> u64 {
	match event {
		Event::String(string) => string.len() as u64,
		Event::Data(data) => data.len() as u64,
		Event::StartArray(Some(count)) => count.saturating_mul(OBJECT_REFERENCE_BYTES),
		Event::StartDictionary(Some(count)) => {
			count.saturating_mul(DICTIONARY_REFERENCES * OBJECT_REFERENCE_BYTES)
		}
		_ => 0,
	}
}
