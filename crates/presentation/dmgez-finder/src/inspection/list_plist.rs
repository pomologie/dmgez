use std::{borrow::Cow, collections::HashSet, io::Cursor as IoCursor, str};

use plist::{Dictionary, Value};

use super::{
	ReadBudget, ReadError, ReadLimits,
	binary_plist::{
		ARRAY_MARKER, DICTIONARY_MARKER, DICTIONARY_REFS_PER_ENTRY, INTEGER_MARKER, MARKER_KIND,
		MARKER_VALUE, OffsetTable, PLIST_MAGIC, integer, object_length,
	},
	leaf::Cursor,
};

#[cfg(test)]
mod tests;

const MAX_DEPTH: usize = 6;
const MAX_EXPANDED_OBJECTS: usize = 2_048;
const MAX_EXPANDED_BYTES: usize = 1 << 20;
const FALSE_MARKER: u8 = 0x08;
const TRUE_MARKER: u8 = 0x09;
const REAL32_MARKER: u8 = 0x22;
const REAL64_MARKER: u8 = 0x23;
const DATA_MARKER: u8 = 0x40;
const BYTE_STRING_MARKER: u8 = 0x50;
const UTF16_STRING_MARKER: u8 = 0x60;
const MAX_INTEGER_POWER: u8 = 4;

pub(super) fn read(blob: &[u8], limits: ReadLimits) -> Result<Dictionary, ReadError> {
	admit(blob, limits)?;
	Value::from_reader(IoCursor::new(blob))?
		.into_dictionary()
		.ok_or(ReadError::PlistProfile)
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Kind {
	Scalar,
	ByteString,
	Utf16String,
	Array,
	Dictionary,
}

impl Kind {
	const fn is_collection(self) -> bool {
		matches!(self, Self::Array | Self::Dictionary)
	}
}

#[derive(Clone, Copy)]
struct Object<'a> {
	kind: Kind,
	payload: &'a [u8],
	encoded_bytes: usize,
}

fn read_objects<'a>(
	table: &OffsetTable<'a>,
	limits: ReadLimits,
) -> Result<Vec<Object<'a>>, ReadError> {
	if table.has_padding {
		return Err(ReadError::PlistLayout);
	}
	let count = table.addresses().len();
	let mut addresses = Vec::new();
	addresses.try_reserve_exact(count)?;
	for (index, offset) in table.addresses().enumerate() {
		let offset = offset?;
		if !(PLIST_MAGIC.len()..table.objects.len()).contains(&offset) {
			return Err(ReadError::PlistLayout);
		}
		addresses.push((offset, index));
	}
	addresses.sort_unstable();
	let mut objects = Vec::new();
	objects.try_reserve_exact(count)?;
	let mut end = table.objects.len();
	for (start, index) in addresses.into_iter().rev() {
		// The next physical offset bounds the current object's encoded payload.
		// Two references may share an index, while offsets must be distinct.
		let bytes = &table.objects[start..end];
		objects.push((index, Object::read(bytes, table.reference_width, limits)?));
		end = start;
	}
	objects.sort_unstable_by_key(|(index, _)| *index);
	Ok(objects.into_iter().map(|(_, object)| object).collect())
}

impl<'a> Object<'a> {
	fn read(
		bytes: &'a [u8],
		reference_width: usize,
		limits: ReadLimits,
	) -> Result<Self, ReadError> {
		let mut input = Cursor::new(bytes);
		let [marker] = input.array().map_err(|_| ReadError::PlistLayout)?;
		let (kind, length) = match marker {
			FALSE_MARKER | TRUE_MARKER => (Kind::Scalar, 0),
			REAL32_MARKER => (Kind::Scalar, size_of::<f32>()),
			REAL64_MARKER => (Kind::Scalar, size_of::<f64>()),
			_ => match marker & MARKER_KIND {
				INTEGER_MARKER if marker & MARKER_VALUE <= MAX_INTEGER_POWER => {
					(Kind::Scalar, 1 << (marker & MARKER_VALUE))
				}
				DATA_MARKER => (Kind::Scalar, object_length(marker, &mut input)?),
				BYTE_STRING_MARKER => (Kind::ByteString, object_length(marker, &mut input)?),
				UTF16_STRING_MARKER => (
					Kind::Utf16String,
					object_length(marker, &mut input)?
						.checked_mul(size_of::<u16>())
						.ok_or(ReadError::PlistLayout)?,
				),
				ARRAY_MARKER | DICTIONARY_MARKER => {
					let entries = object_length(marker, &mut input)?;
					if entries > limits.plist_entries {
						return Err(ReadError::Limit(ReadBudget::PlistEntries));
					}
					let (kind, refs) = if marker & MARKER_KIND == DICTIONARY_MARKER {
						(Kind::Dictionary, DICTIONARY_REFS_PER_ENTRY)
					} else {
						(Kind::Array, 1)
					};
					let length = entries
						.checked_mul(refs * reference_width)
						.ok_or(ReadError::PlistLayout)?;
					(kind, length)
				}
				_ => return Err(ReadError::PlistProfile),
			},
		};
		let payload = input.remaining();
		if payload.len() != length {
			return Err(ReadError::PlistLayout);
		}
		Ok(Self {
			kind,
			payload,
			encoded_bytes: bytes.len(),
		})
	}

	fn key(self) -> Result<Cow<'a, str>, ReadError> {
		match self.kind {
			Kind::ByteString => str::from_utf8(self.payload)
				.map(Cow::Borrowed)
				.map_err(|_| ReadError::PlistLayout),
			Kind::Utf16String => {
				let (units, _) = self.payload.as_chunks::<{ size_of::<u16>() }>();
				char::decode_utf16(units.iter().copied().map(u16::from_be_bytes))
					.collect::<Result<String, _>>()
					.map(Cow::Owned)
					.map_err(|_| ReadError::PlistLayout)
			}
			_ => Err(ReadError::PlistLayout),
		}
	}
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Visit {
	Unseen,
	Visiting,
	Seen,
}

struct Graph<'a> {
	objects: Vec<Object<'a>>,
	visits: Vec<Visit>,
	reference_width: usize,
	remaining_objects: usize,
	remaining_bytes: usize,
}

impl Graph<'_> {
	fn visit(&mut self, index: usize, depth: usize) -> Result<(), ReadError> {
		if depth > MAX_DEPTH {
			return Err(ReadError::Limit(ReadBudget::PlistDepth));
		}
		let object = *self.objects.get(index).ok_or(ReadError::PlistLayout)?;
		if self.visits[index] == Visit::Visiting {
			return Err(ReadError::PlistLayout);
		}
		self.visits[index] = Visit::Visiting;
		self.remaining_objects = self
			.remaining_objects
			.checked_sub(1)
			.ok_or(ReadError::Limit(ReadBudget::PlistExpansion))?;
		self.remaining_bytes = self
			.remaining_bytes
			.checked_sub(object.encoded_bytes)
			.ok_or(ReadError::Limit(ReadBudget::PlistExpansion))?;
		if object.kind.is_collection() {
			for address in object.payload.chunks_exact(self.reference_width) {
				self.visit(integer(address)?, depth + 1)?;
			}
		}
		self.visits[index] = Visit::Seen;
		Ok(())
	}

	fn check_keys(&self) -> Result<(), ReadError> {
		for object in &self.objects {
			if object.kind != Kind::Dictionary {
				continue;
			}
			let bytes = object.payload.len() / DICTIONARY_REFS_PER_ENTRY;
			let mut keys = HashSet::new();
			keys.try_reserve(bytes / self.reference_width)?;
			for key in object.payload[..bytes].chunks_exact(self.reference_width) {
				let key = self
					.objects
					.get(integer(key)?)
					.ok_or(ReadError::PlistLayout)?
					.key()?;
				if !keys.insert(key) {
					return Err(ReadError::PlistLayout);
				}
			}
		}
		Ok(())
	}
}

// Bound both the encoded graph and its fully expanded tree before the general
// plist reader allocates it. Shared references are charged on every visit,
// including shared strings and data values.
fn admit(blob: &[u8], limits: ReadLimits) -> Result<(), ReadError> {
	let table = OffsetTable::read(blob, limits)?;
	let objects = read_objects(&table, limits)?;
	if objects[table.root].kind != Kind::Dictionary {
		return Err(ReadError::PlistProfile);
	}
	let mut graph = Graph {
		visits: vec![Visit::Unseen; objects.len()],
		objects,
		reference_width: table.reference_width,
		remaining_objects: MAX_EXPANDED_OBJECTS,
		remaining_bytes: MAX_EXPANDED_BYTES,
	};
	graph.visit(table.root, 0)?;
	// Sonoma leaves duplicate scalar objects in the table after coalescing
	// their references. Unreferenced collections are outside this profile.
	if graph
		.visits
		.iter()
		.zip(&graph.objects)
		.any(|(&visit, object)| visit == Visit::Unseen && object.kind.is_collection())
	{
		return Err(ReadError::PlistLayout);
	}
	graph.check_keys()
}
