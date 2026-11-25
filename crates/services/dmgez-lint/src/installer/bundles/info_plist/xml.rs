use std::borrow::Cow;

use quick_xml::{
	Reader,
	escape::resolve_xml_entity,
	events::{BytesRef, Event},
};

use super::{Budget, Error, MAX_DEPTH, MAX_EVENTS, MAX_EXPANDED_BYTES, charge, check_cancellation};

const PLIST: &str = "plist";
const DICTIONARY: &str = "dict";
const ARRAY: &str = "array";
const KEY: &str = "key";
const STRING: &str = "string";
const DATA: &str = "data";
const DATE: &str = "date";
const INTEGER: &str = "integer";
const REAL: &str = "real";
const TRUE: &str = "true";
const FALSE: &str = "false";
const INTERNAL_SUBSET: char = '[';
const AMPERSAND: char = '&';
const MARKUP_OPEN: char = '<';
const MARKUP_CLOSE: char = '>';
const MAX_XML_EVENTS: u64 = 4 * MAX_EVENTS;
// The raw XML stack also includes the plist wrapper and a scalar element.
const MAX_XML_DEPTH: u64 = MAX_DEPTH + 2;

/// Validate XML roles before the plist stream merges keys with string values.
/// Rewrite CDATA as escaped text because the stream omits CDATA events.
pub(super) fn validate<'input>(
	bytes: &'input [u8],
	cancelled: &mut impl FnMut() -> bool,
) -> Result<Option<Cow<'input, [u8]>>, Error> {
	let mut reader = Reader::from_reader(bytes);
	reader.config_mut().expand_empty_elements = true;
	let mut frames = Vec::<Element>::new();
	let mut remaining = MAX_XML_EVENTS;
	let (mut document_seen, mut declaration_seen, mut doctype_seen) = (false, false, false);
	let mut rewritten = None;
	let mut pending = 0;
	loop {
		check_cancellation(cancelled)?;
		charge(&mut remaining, 1, Budget::PlistEvents, MAX_XML_EVENTS)?;
		// The logical stream position excludes a UTF-8 BOM. Use physical byte offsets.
		let event_start = bytes.len() - reader.get_ref().len();
		let Ok(event) = reader.read_event() else {
			return Ok(None);
		};
		match event {
			Event::Start(tag) => {
				if tag.attributes().any(|attribute| attribute.is_err()) {
					return Ok(None);
				}
				let Some(child) = Element::from_name(tag.name().as_ref()) else {
					return Ok(None);
				};
				match frames.last_mut() {
					None if !document_seen && child.admits_at_root() => document_seen = true,
					Some(parent) => {
						if !parent.admit_child(child) {
							return Ok(None);
						}
					}
					_ => return Ok(None),
				}
				if frames.len() as u64 == MAX_XML_DEPTH {
					return Err(Error::Limit {
						budget: Budget::PlistDepth,
						limit: MAX_DEPTH,
					});
				}
				frames.push(child);
			}
			Event::End(_) => {
				if !frames.pop().is_some_and(Element::complete) {
					return Ok(None);
				}
			}
			Event::Text(text) => {
				let text = text.as_ref();
				if !text.chars().all(valid_xml_scalar)
					|| (!frames.last().is_some_and(Element::allows_text)
						&& !text.as_bytes().iter().all(u8::is_ascii_whitespace))
				{
					return Ok(None);
				}
			}
			Event::GeneralRef(reference) => {
				if !frames.last().is_some_and(Element::allows_text) || !valid_entity(&reference) {
					return Ok(None);
				}
			}
			Event::CData(text) => {
				if !frames.last().is_some_and(Element::allows_text)
					|| !text.as_ref().chars().all(valid_xml_scalar)
				{
					return Ok(None);
				}
				let Some(prefix) = bytes.get(pending..event_start) else {
					return Ok(None);
				};
				let output = rewritten.get_or_insert_with(Vec::new);
				append(output, prefix)?;
				append_escaped(output, text.as_ref())?;
				pending = bytes.len() - reader.get_ref().len();
			}
			Event::Decl(_) if !document_seen && !doctype_seen && !declaration_seen => {
				declaration_seen = true
			}
			Event::DocType(content)
				if !document_seen
					&& !doctype_seen
					&& !content.as_ref().contains(INTERNAL_SUBSET) =>
			{
				doctype_seen = true
			}
			Event::Comment(_) => {}
			Event::Eof if document_seen && frames.is_empty() => {
				return match rewritten {
					Some(mut output) => {
						let Some(suffix) = bytes.get(pending..) else {
							return Ok(None);
						};
						append(&mut output, suffix)?;
						Ok(Some(Cow::Owned(output)))
					}
					None => Ok(Some(Cow::Borrowed(bytes))),
				};
			}
			_ => return Ok(None),
		}
	}
}

#[derive(Clone, Copy)]
enum Element {
	Document { has_dictionary: bool },
	Dictionary { expects_key: bool },
	Array,
	Key,
	Text,
	Boolean,
}

impl Element {
	fn from_name(name: &str) -> Option<Self> {
		let element = match name {
			PLIST => Self::Document {
				has_dictionary: false,
			},
			DICTIONARY => Self::Dictionary { expects_key: true },
			ARRAY => Self::Array,
			KEY => Self::Key,
			STRING | DATA | DATE | INTEGER | REAL => Self::Text,
			TRUE | FALSE => Self::Boolean,
			_ => return None,
		};
		Some(element)
	}

	const fn admits_at_root(self) -> bool {
		matches!(self, Self::Document { .. } | Self::Dictionary { .. })
	}

	const fn is_value(self) -> bool {
		!matches!(self, Self::Key | Self::Document { .. })
	}

	fn admit_child(&mut self, child: Self) -> bool {
		match self {
			Self::Document { has_dictionary }
				if !*has_dictionary && matches!(child, Self::Dictionary { .. }) =>
			{
				*has_dictionary = true;
				true
			}
			Self::Dictionary { expects_key }
				if (*expects_key && matches!(child, Self::Key))
					|| (!*expects_key && child.is_value()) =>
			{
				*expects_key = !*expects_key;
				true
			}
			Self::Array => child.is_value(),
			_ => false,
		}
	}

	const fn complete(self) -> bool {
		!matches!(
			self,
			Self::Document {
				has_dictionary: false
			} | Self::Dictionary { expects_key: false }
		)
	}

	const fn allows_text(&self) -> bool {
		matches!(self, Self::Text | Self::Key)
	}
}

fn valid_entity(entity: &BytesRef<'_>) -> bool {
	match entity.resolve_char_ref() {
		Ok(Some(scalar)) => valid_xml_scalar(scalar),
		Ok(None) => resolve_xml_entity(entity.as_ref()).is_some(),
		Err(_) => false,
	}
}

fn valid_xml_scalar(scalar: char) -> bool {
	matches!(scalar, '\t' | '\n' | '\r' | '\u{20}'..='\u{FFFD}' | '\u{10000}'..='\u{10FFFF}')
}

fn append_escaped(output: &mut Vec<u8>, value: &str) -> Result<(), Error> {
	let mut pending = 0;
	for (offset, character) in value.char_indices() {
		let replacement = match character {
			AMPERSAND => b"&amp;".as_slice(),
			MARKUP_OPEN => b"&lt;".as_slice(),
			MARKUP_CLOSE => b"&gt;".as_slice(),
			_ => continue,
		};
		append(output, &value.as_bytes()[pending..offset])?;
		append(output, replacement)?;
		pending = offset + character.len_utf8();
	}
	append(output, &value.as_bytes()[pending..])
}

fn append(output: &mut Vec<u8>, bytes: &[u8]) -> Result<(), Error> {
	if bytes.len() as u64 > MAX_EXPANDED_BYTES.saturating_sub(output.len() as u64) {
		return Err(Error::Limit {
			budget: Budget::PlistExpandedBytes,
			limit: MAX_EXPANDED_BYTES,
		});
	}
	output.try_reserve(bytes.len())?;
	output.extend_from_slice(bytes);
	Ok(())
}
