use quick_xml::{Reader, events::Event};

use super::MAX_PLIST_BYTES;

const MAX_DEPTH: usize = 64;
const MAX_EVENTS: usize = 16 * 1024;
const INTERNAL_SUBSET: char = '[';

pub(super) fn count_keys(bytes: &[u8]) -> Option<usize> {
	if bytes.len() > MAX_PLIST_BYTES {
		return None;
	}
	let mut reader = Reader::from_reader(bytes);
	reader.config_mut().expand_empty_elements = true;
	let mut frames = Vec::new();
	let mut document = false;
	let mut declaration = false;
	let mut doctype = false;
	let mut keys = 0;
	for _ in 0..MAX_EVENTS {
		match reader.read_event().ok()? {
			Event::Start(tag) => {
				if tag.attributes().any(|attribute| attribute.is_err()) {
					return None;
				}
				let child = Frame::from_name(tag.name().as_ref())?;
				match frames.last_mut() {
					Some(parent) => admit_child(parent, child)?,
					None if !document && matches!(child, Frame::Plist(false)) => document = true,
					_ => return None,
				}
				if frames.len() == MAX_DEPTH {
					return None;
				}
				keys += usize::from(matches!(child, Frame::Key));
				frames.push(child);
			}
			Event::End(_) => {
				if matches!(
					frames.pop()?,
					Frame::Plist(false) | Frame::Dictionary(false)
				) {
					return None;
				}
			}
			Event::Decl(_) if !document && !declaration && !doctype => declaration = true,
			Event::DocType(content)
				if !document && !doctype && !content.as_ref().contains(INTERNAL_SUBSET) =>
			{
				doctype = true;
			}
			Event::Text(text)
				if !frames.last().is_some_and(Frame::allows_text)
					&& !text.as_ref().as_bytes().iter().all(u8::is_ascii_whitespace) =>
			{
				return None;
			}
			Event::GeneralRef(_) if !frames.last().is_some_and(Frame::allows_text) => return None,
			Event::CData(_)
			| Event::PI(_)
			| Event::Empty(_)
			| Event::Decl(_)
			| Event::DocType(_) => {
				return None;
			}
			Event::Eof => return (document && frames.is_empty()).then_some(keys),
			_ => {}
		}
	}
	None
}

#[derive(Clone, Copy)]
enum Frame {
	Plist(bool),
	Dictionary(bool),
	Array,
	Key,
	Scalar,
	Boolean,
}

impl Frame {
	fn from_name(name: &str) -> Option<Self> {
		Some(match name {
			"plist" => Self::Plist(false),
			"dict" => Self::Dictionary(true),
			"array" => Self::Array,
			"key" => Self::Key,
			"string" | "data" | "date" | "integer" | "real" => Self::Scalar,
			"true" | "false" => Self::Boolean,
			_ => return None,
		})
	}

	const fn allows_text(&self) -> bool {
		matches!(self, Self::Key | Self::Scalar)
	}
}

fn admit_child(parent: &mut Frame, child: Frame) -> Option<()> {
	let is_key = matches!(child, Frame::Key);
	if matches!(child, Frame::Plist(_)) {
		return None;
	}
	match parent {
		Frame::Plist(seen) if !*seen && matches!(child, Frame::Dictionary(_)) => *seen = true,
		Frame::Dictionary(expects_key) if *expects_key == is_key => *expects_key = !*expects_key,
		Frame::Array if !is_key => {}
		_ => return None,
	}
	Some(())
}
