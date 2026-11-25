//! Structural checks for the UDIF embedded signature that holds a ticket.

use crate::Error;

// Apple Security's sigblob.h and codedirectory.h define these wire values.
// diskimagerep.cpp reads the ticket from the embedded signature.
const EMBEDDED_SIGNATURE_MAGIC: u32 = 0xfade_0cc0;
pub(super) const TICKET_SLOT: u32 = 0x0001_0002;
const BLOB_WRAPPER_MAGIC: u32 = 0xfade_0b01;
const WORD_BYTES: usize = size_of::<u32>();
const BLOB_HEADER_BYTES: usize = 2 * WORD_BYTES;
const SUPERBLOB_HEADER_BYTES: usize = 3 * WORD_BYTES;
const INDEX_ENTRY_BYTES: usize = 2 * WORD_BYTES;
const MAX_COMPONENTS: usize = 64;
/// Maximum size of an embedded UDIF signature accepted by the envelope parser.
pub const MAX_EMBEDDED_SIGNATURE_BYTES: usize = 16 * 1024 * 1024;

/// Reports whether a bounded, well-formed embedded signature has a ticket slot.
///
/// A present ticket still requires cryptographic verification.
///
/// # Errors
///
/// Returns an error for malformed signature framing or resource limits.
pub fn has_stapled_ticket(signature: &[u8]) -> Result<bool, Error> {
	Ok(components(signature)?
		.iter()
		.any(|component| component.slot == TICKET_SLOT))
}

/// Checks that the signature gained a single ticket and kept every other slot
/// byte for byte.
///
/// Index entries and physical component order may change. Each frame must have
/// unique slots and a contiguous, exhaustive component layout. The ticket
/// payload is opaque and requires a native validity check before publication.
///
/// # Errors
///
/// Returns an error for malformed signature framing, resource limits, a
/// preexisting or missing ticket, or a change to any original component.
pub fn validate_ticket_addition(before: &[u8], after: &[u8]) -> Result<(), Error> {
	let before = components(before)?;
	let after = components(after)?;
	if before.iter().any(|component| component.slot == TICKET_SLOT) {
		return Err(Error::Unsupported("input UDIF already has a ticket"));
	}
	if !after.iter().any(|component| component.slot == TICKET_SLOT) {
		return Err(Error::Corrupt("stapled UDIF lacks the ticket slot"));
	}
	let unstapled = before.iter().map(Component::contents);
	let retained = after
		.iter()
		.filter(|component| component.slot != TICKET_SLOT)
		.map(Component::contents);
	if !unstapled.eq(retained) {
		return Err(Error::Corrupt("stapling changed UDIF signature components"));
	}
	Ok(())
}

pub(super) struct Component<'signature> {
	pub(super) slot: u32,
	offset: usize,
	pub(super) data: &'signature [u8],
}

impl<'signature> Component<'signature> {
	fn contents(&self) -> (u32, &'signature [u8]) {
		(self.slot, self.data)
	}
}

pub(super) fn components(signature: &[u8]) -> Result<Vec<Component<'_>>, Error> {
	if signature.len() > MAX_EMBEDDED_SIGNATURE_BYTES {
		return Err(Error::ResourceLimit("UDIF embedded signature bytes"));
	}
	if word(signature, 0)? != EMBEDDED_SIGNATURE_MAGIC
		|| size(signature, WORD_BYTES)? != signature.len()
	{
		return Err(Error::Corrupt("invalid UDIF embedded signature header"));
	}
	let count = size(signature, BLOB_HEADER_BYTES)?;
	if count == 0 {
		return Err(Error::Corrupt("empty UDIF embedded signature index"));
	}
	if count > MAX_COMPONENTS {
		return Err(Error::ResourceLimit("UDIF embedded signature components"));
	}
	let payload_offset = SUPERBLOB_HEADER_BYTES + count * INDEX_ENTRY_BYTES;
	let entries = signature
		.get(SUPERBLOB_HEADER_BYTES..payload_offset)
		.ok_or(Error::Corrupt("truncated UDIF embedded signature index"))?;
	let mut components = Vec::with_capacity(count);
	for entry in entries.as_chunks::<INDEX_ENTRY_BYTES>().0 {
		let slot = word(entry, 0)?;
		let offset = size(entry, WORD_BYTES)?;
		let data = read_component(signature, offset, payload_offset)?;
		if slot == TICKET_SLOT
			&& (word(data, 0)? != BLOB_WRAPPER_MAGIC || data.len() == BLOB_HEADER_BYTES)
		{
			return Err(Error::Corrupt("invalid UDIF ticket wrapper"));
		}
		components.push(Component { slot, offset, data });
	}

	components.sort_unstable_by_key(|component| component.offset);
	let mut expected_offset = payload_offset;
	for component in &components {
		if component.offset != expected_offset {
			return Err(Error::Corrupt("noncontiguous UDIF signature components"));
		}
		expected_offset += component.data.len();
	}
	if expected_offset != signature.len() {
		return Err(Error::Corrupt("unindexed bytes in UDIF signature"));
	}
	components.sort_unstable_by_key(|component| component.slot);
	if components
		.windows(2)
		.any(|pair| pair[0].slot == pair[1].slot)
	{
		return Err(Error::Corrupt("duplicate UDIF signature slot"));
	}
	Ok(components)
}

fn read_component(signature: &[u8], offset: usize, payload_offset: usize) -> Result<&[u8], Error> {
	if offset < payload_offset {
		return Err(Error::Corrupt(
			"UDIF signature component overlaps the index",
		));
	}
	let tail = signature.get(offset..).ok_or(Error::Corrupt(
		"UDIF signature component offset is out of range",
	))?;
	let length = size(tail, WORD_BYTES)?;
	if length < BLOB_HEADER_BYTES {
		return Err(Error::Corrupt("short UDIF signature component"));
	}
	tail.get(..length).ok_or(Error::Corrupt(
		"UDIF signature component length is out of range",
	))
}

pub(super) fn word(bytes: &[u8], offset: usize) -> Result<u32, Error> {
	bytes
		.get(offset..)
		.and_then(<[u8]>::first_chunk::<WORD_BYTES>)
		.copied()
		.map(u32::from_be_bytes)
		.ok_or(Error::Corrupt("truncated UDIF embedded signature field"))
}

pub(super) fn size(bytes: &[u8], offset: usize) -> Result<usize, Error> {
	usize::try_from(word(bytes, offset)?)
		.map_err(|_| Error::ResourceLimit("UDIF embedded signature field"))
}
