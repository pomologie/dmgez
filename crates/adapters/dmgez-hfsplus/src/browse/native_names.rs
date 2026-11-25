use dmgez_vfs::volume::{LookupName, NativeNameKey, NativeNameProfile};
use hfsplus_core::names::{
	MAX_NAME_UNITS, NameCollation, NameError, NameFragment as HfsNameFragment,
};

use super::Error;

/// A normalized filename fragment compared with the volume's catalog collation.
#[derive(Debug)]
pub struct NameFragment(HfsNameFragment);

impl NameFragment {
	/// Maximum UTF-16 length of the decomposed fragment.
	pub const MAX_UNITS: usize = HfsNameFragment::MAX_UNITS;

	/// Uses HFS+ decomposition and the selected volume's native name profile.
	///
	/// Returns `None` for an empty or entirely ignorable fragment.
	///
	/// # Errors
	/// Returns an error for an unsupported collation, an overlong fragment, or
	/// failed allocation.
	pub fn new(profile: Option<NativeNameProfile>, fragment: &str) -> Result<Option<Self>, Error> {
		let collation = match profile {
			Some(NativeNameProfile::HfsUnicodeFolded) => NameCollation::CaseFolded,
			Some(NativeNameProfile::HfsXBinary) => NameCollation::Binary,
			_ => return Err(Error::LookupEncoding),
		};
		Ok(HfsNameFragment::new(collation, fragment)?.map(Self))
	}

	/// Tests a stored UTF-16 name without changing the original units.
	///
	/// # Errors
	/// Returns an error if the engine cannot compare this stored name.
	pub fn matches(&self, name: &[u16]) -> Result<bool, Error> {
		Ok(self.0.matches(name)?)
	}
}

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
