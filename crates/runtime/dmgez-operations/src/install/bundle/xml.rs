use std::{borrow::Cow, error::Error as StdError};

use quick_xml::{
	Reader,
	escape::{escape, resolve_xml_entity},
	events::Event,
};

use super::{Error, MAX_DEPTH, MAX_EVENTS, MAX_EXPANDED_BYTES, check_cancellation};

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
const MAX_XML_EVENTS: usize = 4 * MAX_EVENTS;
// The wrapper and scalar each add one level above the plist collection budget.
const MAX_XML_DEPTH: usize = MAX_DEPTH + 2;

#[derive(Clone, Copy)]
enum Element {
	Plist { dictionary_seen: bool },
	Dictionary { expects_key: bool },
	Array,
	Key,
	Scalar,
	Boolean,
}

impl Element {
	fn parse(name: &str) -> Option<Self> {
		Some(match name {
			PLIST => Self::Plist {
				dictionary_seen: false,
			},
			DICTIONARY => Self::Dictionary { expects_key: true },
			ARRAY => Self::Array,
			KEY => Self::Key,
			STRING | DATA | DATE | INTEGER | REAL => Self::Scalar,
			TRUE | FALSE => Self::Boolean,
			_ => return None,
		})
	}

	fn accepts(&mut self, child: Self) -> bool {
		let value = !matches!(child, Self::Plist { .. } | Self::Key);
		match self {
			Self::Plist { dictionary_seen }
				if !*dictionary_seen && matches!(child, Self::Dictionary { .. }) =>
			{
				*dictionary_seen = true;
				true
			}
			Self::Dictionary { expects_key }
				if (*expects_key && matches!(child, Self::Key)) || (!*expects_key && value) =>
			{
				*expects_key = !*expects_key;
				true
			}
			Self::Array => value,
			_ => false,
		}
	}

	fn complete(self) -> bool {
		!matches!(
			self,
			Self::Dictionary { expects_key: false }
				| Self::Plist {
					dictionary_seen: false
				}
		)
	}

	fn allows_text(&self) -> bool {
		matches!(self, Self::Key | Self::Scalar)
	}
}

// Validate XML roles before the plist event reader merges keys and strings.
// Normalize CDATA to escaped text so the event reader preserves its contents.
pub(super) fn normalize<'input, E: StdError + 'static>(
	bytes: &'input [u8],
	cancelled: &mut impl FnMut() -> bool,
) -> Result<Cow<'input, [u8]>, Error<E>> {
	let mut reader = Reader::from_reader(bytes);
	reader.config_mut().expand_empty_elements = true;
	let mut stack = Vec::<Element>::new();
	let (mut started, mut declared, mut doctype) = (false, false, false);
	let mut normalized = None;
	let mut copied = 0;
	for _ in 0..MAX_XML_EVENTS {
		check_cancellation(cancelled)?;
		// Use the physical input position, which includes any UTF-8 BOM.
		let event_start = bytes.len() - reader.get_ref().len();
		match reader.read_event().map_err(|_| Error::Info)? {
			Event::Start(tag) => {
				if tag.attributes().any(|attribute| attribute.is_err()) {
					return Err(Error::Info);
				}
				let child = Element::parse(tag.name().as_ref()).ok_or(Error::Info)?;
				match stack.last_mut() {
					Some(parent) => {
						if !parent.accepts(child) {
							return Err(Error::Info);
						}
					}
					None if !started
						&& matches!(child, Element::Plist { .. } | Element::Dictionary { .. }) =>
					{
						started = true;
					}
					_ => return Err(Error::Info),
				}
				if stack.len() >= MAX_XML_DEPTH {
					return Err(Error::InfoLimit);
				}
				stack.push(child);
			}
			Event::End(_) if stack.pop().is_some_and(Element::complete) => {}
			Event::Text(text)
				if text.as_ref().chars().all(valid_xml_character)
					&& (stack.last().is_some_and(Element::allows_text)
						|| text.as_ref().as_bytes().iter().all(u8::is_ascii_whitespace)) => {}
			Event::GeneralRef(reference) if stack.last().is_some_and(Element::allows_text) => {
				let valid = match reference.resolve_char_ref() {
					Ok(Some(character)) => valid_xml_character(character),
					Ok(None) => resolve_xml_entity(reference.as_ref()).is_some(),
					Err(_) => false,
				};
				if !valid {
					return Err(Error::Info);
				}
			}
			Event::CData(value)
				if stack.last().is_some_and(Element::allows_text)
					&& value.as_ref().chars().all(valid_xml_character) =>
			{
				let output = normalized.get_or_insert_with(Vec::new);
				append(output, bytes.get(copied..event_start).ok_or(Error::Info)?)?;
				append(output, escape(value.as_ref()).as_bytes())?;
				copied = bytes.len() - reader.get_ref().len();
			}
			Event::Comment(_) => {}
			Event::Decl(_) if !started && !doctype && !declared => declared = true,
			Event::DocType(content)
				if !started && !doctype && !content.as_ref().contains(INTERNAL_SUBSET) =>
			{
				doctype = true;
			}
			Event::Eof if started && stack.is_empty() => {
				return match normalized {
					Some(mut output) => {
						append(&mut output, bytes.get(copied..).ok_or(Error::Info)?)?;
						Ok(Cow::Owned(output))
					}
					None => Ok(Cow::Borrowed(bytes)),
				};
			}
			_ => return Err(Error::Info),
		}
	}
	Err(Error::InfoLimit)
}

fn append<E: StdError + 'static>(output: &mut Vec<u8>, bytes: &[u8]) -> Result<(), Error<E>> {
	if (output.len() as u64).saturating_add(bytes.len() as u64) > MAX_EXPANDED_BYTES {
		return Err(Error::InfoLimit);
	}
	output.try_reserve(bytes.len())?;
	output.extend_from_slice(bytes);
	Ok(())
}

fn valid_xml_character(character: char) -> bool {
	matches!(character, '\t' | '\n' | '\r' | '\u{20}'..='\u{FFFD}' | '\u{10000}'..='\u{10FFFF}')
}
