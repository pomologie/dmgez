use std::{borrow::Cow, collections::BTreeSet, io::Cursor};

use dmgez_config::VirtualPath;
use plist::stream::{BinaryReader, Event, XmlReader};

use super::{Budget, BundleInfoProblem, Error, check_cancellation};

mod xml;

const BINARY_PLIST_MAGIC: &[u8; 8] = b"bplist00";
const BUNDLE_EXECUTABLE_KEY: &str = "CFBundleExecutable";
const PATH_SEPARATOR: char = '/';
const MAX_EVENTS: u64 = 16_384;
const MAX_DEPTH: u64 = 32;
const MAX_EXPANDED_BYTES: u64 = 16 << 20;

pub(super) fn executable(
	bytes: &[u8],
	cancelled: &mut impl FnMut() -> bool,
) -> Result<Result<String, BundleInfoProblem>, Error> {
	if bytes.starts_with(BINARY_PLIST_MAGIC) {
		inspect(BinaryReader::new(Cursor::new(bytes)), cancelled)
	} else if let Some(xml) = xml::validate(bytes, cancelled)? {
		inspect(XmlReader::new(xml.as_ref()), cancelled)
	} else {
		Ok(Err(BundleInfoProblem::InvalidPlist))
	}
}

fn inspect(
	events: impl Iterator<Item = Result<Event<'static>, plist::Error>>,
	cancelled: &mut impl FnMut() -> bool,
) -> Result<Result<String, BundleInfoProblem>, Error> {
	let mut stack = Vec::new();
	let mut remaining_events = MAX_EVENTS;
	let mut remaining_bytes = MAX_EXPANDED_BYTES;
	let mut root_seen = false;
	let mut executable = Err(BundleInfoProblem::MissingExecutable);
	for event in events {
		check_cancellation(cancelled)?;
		charge(&mut remaining_events, 1, Budget::PlistEvents, MAX_EVENTS)?;
		let Ok(event) = event else {
			return Ok(Err(BundleInfoProblem::InvalidPlist));
		};
		charge(
			&mut remaining_bytes,
			expanded_bytes(&event),
			Budget::PlistExpandedBytes,
			MAX_EXPANDED_BYTES,
		)?;
		if stack.is_empty() {
			if root_seen || !matches!(event, Event::StartDictionary(_)) {
				return Ok(Err(BundleInfoProblem::InvalidPlist));
			}
			root_seen = true;
			push(&mut stack, Frame::dictionary())?;
			continue;
		}
		if matches!(event, Event::EndCollection) {
			if matches!(
				stack.pop(),
				Some(Frame::Dictionary {
					value_for: Some(_),
					..
				})
			) {
				return Ok(Err(BundleInfoProblem::InvalidPlist));
			}
			continue;
		}

		let is_root = stack.len() == 1;
		if let Some(Frame::Dictionary { keys, value_for }) = stack.last_mut() {
			if let Some(is_executable) = value_for.take() {
				if is_executable {
					executable = match &event {
						Event::String(name) if safe_component(name) => Ok(name.to_string()),
						_ => Err(BundleInfoProblem::InvalidExecutable),
					};
				}
			} else {
				let Event::String(key) = event else {
					return Ok(Err(BundleInfoProblem::InvalidPlist));
				};
				*value_for = Some(is_root && key == BUNDLE_EXECUTABLE_KEY);
				if !keys.insert(key) {
					return Ok(Err(BundleInfoProblem::InvalidPlist));
				}
				continue;
			}
		}
		match event {
			Event::StartArray(_) => push(&mut stack, Frame::Array)?,
			Event::StartDictionary(_) => push(&mut stack, Frame::dictionary())?,
			_ => {}
		}
	}
	if root_seen && stack.is_empty() {
		Ok(executable)
	} else {
		Ok(Err(BundleInfoProblem::InvalidPlist))
	}
}

enum Frame {
	Array,
	Dictionary {
		keys: BTreeSet<Cow<'static, str>>,
		value_for: Option<bool>,
	},
}

impl Frame {
	fn dictionary() -> Self {
		Self::Dictionary {
			keys: BTreeSet::new(),
			value_for: None,
		}
	}
}

fn safe_component(name: &str) -> bool {
	!name.is_empty()
		&& !name.contains(PATH_SEPARATOR)
		&& format!("{PATH_SEPARATOR}{name}")
			.parse::<VirtualPath>()
			.is_ok()
}

fn expanded_bytes(event: &Event<'_>) -> u64 {
	const REFERENCE_BYTES: u64 = size_of::<u64>() as u64;
	const REFERENCES_PER_PAIR: u64 = 2;
	match event {
		Event::String(value) => value.len() as u64,
		Event::Data(value) => value.len() as u64,
		Event::StartArray(Some(length)) => length.saturating_mul(REFERENCE_BYTES),
		Event::StartDictionary(Some(length)) => {
			length.saturating_mul(REFERENCE_BYTES * REFERENCES_PER_PAIR)
		}
		_ => 0,
	}
}

fn push(stack: &mut Vec<Frame>, frame: Frame) -> Result<(), Error> {
	if stack.len() as u64 == MAX_DEPTH {
		return Err(Error::Limit {
			budget: Budget::PlistDepth,
			limit: MAX_DEPTH,
		});
	}
	stack.push(frame);
	Ok(())
}

fn charge(remaining: &mut u64, amount: u64, budget: Budget, limit: u64) -> Result<(), Error> {
	*remaining = remaining
		.checked_sub(amount)
		.ok_or(Error::Limit { budget, limit })?;
	Ok(())
}

#[cfg(test)]
mod tests;
