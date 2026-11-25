//! Structural admission for the single-directory SHA-256 UDIF profile.

use crate::{
	Error,
	signature_ticket::{TICKET_SLOT, components, size as offset, word},
};

// Wire fields and virtual slot numbers follow Apple Security's codedirectory.h.
const DIRECTORY_SLOT: u32 = 0;
const REQUIREMENTS_SLOT: u32 = 2;
const CMS_SLOT: u32 = 0x0001_0000;
const CODE_DIRECTORY_MAGIC: u32 = 0xfade_0c02;
const REQUIREMENTS_SET_MAGIC: u32 = 0xfade_0c01;
const SCATTER_VERSION: u32 = 0x0002_0100;
const MAGIC_FIELD: usize = 0;
const VERSION_FIELD: usize = 8;
const HASH_OFFSET_FIELD: usize = 16;
const IDENTIFIER_OFFSET_FIELD: usize = 20;
const SPECIAL_COUNT_FIELD: usize = 24;
const CODE_COUNT_FIELD: usize = 28;
const CODE_LIMIT_FIELD: usize = 32;
const HASH_SIZE_FIELD: usize = 36;
const HASH_TYPE_FIELD: usize = 37;
const PLATFORM_FIELD: usize = 38;
const PAGE_EXPONENT_FIELD: usize = 39;
const RESERVED_FIELD: usize = 40;
const SCATTER_OFFSET_FIELD: usize = 44;
const DIRECTORY_HEADER_BYTES: usize = SCATTER_OFFSET_FIELD + size_of::<u32>();
const REQUIREMENTS_SET_HEADER_BYTES: usize = 3 * size_of::<u32>();
const SHA256_TYPE: u8 = 2;
const SHA256_BYTES: usize = 32;
const SPECIAL_COUNT: u32 = 6;
const CODE_COUNT: u32 = 1;
const GENERIC_PLATFORM: u8 = 0;
const UNPAGED: u8 = 0;

/// Exact digest inputs and stored SHA-256 hashes from an embedded CodeDirectory.
///
/// The caller must match the body length against the UDIF signature offset
/// and compute the body, normalized trailer, and requirements digests.
/// Certificate trust, CMS authenticity and ticket validity require separate
/// checks.
#[derive(Debug)]
#[non_exhaustive]
pub struct EmbeddedCodeDirectory<'signature> {
	/// Entire CodeDirectory blob, including its header.
	pub directory: &'signature [u8],
	/// Number of file bytes preceding the signature that are covered by the body hash.
	pub signing_limit: u64,
	/// Stored digest of the file bytes preceding the signature.
	pub body_hash: &'signature [u8; SHA256_BYTES],
	/// Stored digest of the UDIF trailer with its signature-length field cleared.
	pub trailer_hash: &'signature [u8; SHA256_BYTES],
	/// Stored digest of the entire requirements set blob.
	pub requirements_hash: &'signature [u8; SHA256_BYTES],
	/// Entire, opaque requirements set blob, including its header.
	pub requirements: &'signature [u8],
}

/// Extracts the CodeDirectory digest fields for a bounded UDIF signature.
///
/// Admits one version `0x20100` CodeDirectory with full SHA-256, one unpaged
/// body hash, and six special slots. Only the requirements and normalized
/// UDIF trailer hashes are used among the special slots.
///
/// # Errors
///
/// Returns an error for an unsupported signature profile, malformed framing
/// or offsets, or a signature that exceeds envelope limits.
pub fn embedded_code_directory(signature: &[u8]) -> Result<EmbeddedCodeDirectory<'_>, Error> {
	let components = components(signature)?;
	let mut directory = None;
	let mut requirements = None;
	for component in components {
		match component.slot {
			DIRECTORY_SLOT => directory = Some(component.data),
			REQUIREMENTS_SLOT => requirements = Some(component.data),
			CMS_SLOT | TICKET_SLOT => {}
			_ => return Err(Error::Unsupported("unknown UDIF signature component slot")),
		}
	}
	let directory = directory.ok_or(Error::Corrupt("missing UDIF CodeDirectory"))?;
	let hashes = directory_hashes(directory)?;
	let requirements = requirements.ok_or(Error::Corrupt("missing UDIF requirements set"))?;
	if requirements.len() < REQUIREMENTS_SET_HEADER_BYTES
		|| word(requirements, MAGIC_FIELD)? != REQUIREMENTS_SET_MAGIC
	{
		return Err(Error::Corrupt("invalid UDIF requirements set header"));
	}
	Ok(EmbeddedCodeDirectory {
		directory,
		signing_limit: u64::from(word(directory, CODE_LIMIT_FIELD)?),
		body_hash: hashes.body,
		trailer_hash: hashes.trailer,
		requirements_hash: hashes.requirements,
		requirements,
	})
}

struct DirectoryHashes<'directory> {
	body: &'directory [u8; SHA256_BYTES],
	trailer: &'directory [u8; SHA256_BYTES],
	requirements: &'directory [u8; SHA256_BYTES],
}

fn directory_hashes(directory: &[u8]) -> Result<DirectoryHashes<'_>, Error> {
	if word(directory, MAGIC_FIELD)? != CODE_DIRECTORY_MAGIC {
		return Err(Error::Corrupt("invalid UDIF CodeDirectory magic"));
	}
	if word(directory, VERSION_FIELD)? != SCATTER_VERSION {
		return Err(Error::Unsupported("UDIF CodeDirectory version"));
	}
	if directory.len() < DIRECTORY_HEADER_BYTES {
		return Err(Error::Corrupt("truncated UDIF CodeDirectory header"));
	}
	if usize::from(directory[HASH_SIZE_FIELD]) != SHA256_BYTES
		|| directory[HASH_TYPE_FIELD] != SHA256_TYPE
		|| directory[PLATFORM_FIELD] != GENERIC_PLATFORM
		|| directory[PAGE_EXPONENT_FIELD] != UNPAGED
		|| word(directory, SPECIAL_COUNT_FIELD)? != SPECIAL_COUNT
		|| word(directory, CODE_COUNT_FIELD)? != CODE_COUNT
		|| word(directory, SCATTER_OFFSET_FIELD)? != 0
	{
		return Err(Error::Unsupported("UDIF CodeDirectory digest geometry"));
	}
	if word(directory, RESERVED_FIELD)? != 0 || word(directory, CODE_LIMIT_FIELD)? == 0 {
		return Err(Error::Corrupt("invalid UDIF CodeDirectory header fields"));
	}

	let code_offset = offset(directory, HASH_OFFSET_FIELD)?;
	let special_offset = code_offset
		.checked_sub(SPECIAL_COUNT as usize * SHA256_BYTES)
		.ok_or(Error::Corrupt(
			"invalid UDIF CodeDirectory special hash offset",
		))?;
	let code_end = code_offset
		.checked_add(SHA256_BYTES)
		.filter(|&end| end == directory.len())
		.ok_or(Error::Corrupt("invalid UDIF CodeDirectory body hash range"))?;
	let identifier_offset = offset(directory, IDENTIFIER_OFFSET_FIELD)?;
	let identifier = directory
		.get(identifier_offset..special_offset)
		.filter(|_| identifier_offset >= DIRECTORY_HEADER_BYTES)
		.ok_or(Error::Corrupt(
			"invalid UDIF CodeDirectory identifier range",
		))?;
	if identifier.first().copied().is_none_or(|byte| byte == 0) || !identifier.contains(&0) {
		return Err(Error::Corrupt("invalid UDIF CodeDirectory identifier"));
	}

	// The six special hashes precede the code hash in descending slot order.
	let hashes = directory[special_offset..code_end]
		.as_chunks::<SHA256_BYTES>()
		.0;
	let [
		trailer,
		entitlement,
		application,
		resources,
		requirements,
		info,
		body,
	] = hashes
	else {
		return Err(Error::Corrupt("invalid UDIF CodeDirectory hash table"));
	};
	if [entitlement, application, resources, info]
		.into_iter()
		.any(|hash| hash.iter().any(|&byte| byte != 0))
	{
		return Err(Error::Unsupported(
			"additional UDIF CodeDirectory special hashes",
		));
	}
	Ok(DirectoryHashes {
		body,
		trailer,
		requirements,
	})
}
