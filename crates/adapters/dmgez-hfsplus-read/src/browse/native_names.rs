use dmgez_vfs::volume::{LookupName, NativeNameKey, NativeNameProfile};
use hfsplus_core::names::{MAX_NAME_UNITS, NameCollation, NameError};

use super::Error;

pub(super) const fn profile(collation: NameCollation) -> Option<NativeNameProfile> {
	match collation {
		NameCollation::CaseFolded => Some(NativeNameProfile::HfsUnicodeFolded),
		NameCollation::Binary => Some(NativeNameProfile::HfsXBinary),
		_ => None,
	}
}

pub(super) fn key(
	collation: NameCollation,
	name: LookupName<'_>,
) -> Result<Option<NativeNameKey>, Error> {
	if profile(collation).is_none() {
		return Ok(None);
	}
	let mut utf16 = [0; MAX_NAME_UNITS];
	let units = match name {
		LookupName::Utf8(name) => encode_stored_name(name, &mut utf16)?,
		LookupName::StoredUtf16(units) => units,
		_ => return Err(Error::LookupEncoding),
	};
	let weights = collation.key(units)?;
	let mut bytes = Vec::new();
	bytes.try_reserve_exact(weights.len() * size_of::<u16>())?;
	for weight in weights {
		// Big endian preserves unsigned UTF-16 comparator order in byte keys.
		bytes.extend_from_slice(&weight.to_be_bytes());
	}
	Ok(Some(NativeNameKey::new(bytes)))
}

fn encode_stored_name<'buffer>(
	name: &str,
	utf16: &'buffer mut [u16; MAX_NAME_UNITS],
) -> Result<&'buffer [u16], NameError> {
	let mut len = 0;
	for unit in name.encode_utf16() {
		*utf16.get_mut(len).ok_or(NameError::TooLong)? = unit;
		len += 1;
	}
	Ok(&utf16[..len])
}

#[cfg(test)]
mod tests;
