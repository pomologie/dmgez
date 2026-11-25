use std::convert::Infallible;

use super::{NameError, NameProfile, charge_child_path, validate_component};
use crate::extract::{NameBudget, NamePlanError};

const COMPONENT_BYTES: usize = 12;
const COMPONENTS: usize = 8;

fn posix_component(bytes: &[u8], limit: usize) -> Result<(), NameError> {
	if bytes.len() > limit {
		return Err(NameError::TooLong);
	}
	if bytes.is_empty() || bytes == b"." || bytes == b".." {
		return Err(NameError::NotComponent);
	}
	for &byte in bytes {
		if byte == b'/' || byte == 0 {
			return Err(NameError::NotComponent);
		}
	}
	Ok(())
}

fn portable_component(bytes: &[u8], limit: usize) -> Result<(), NameError> {
	posix_component(bytes, limit)?;
	if bytes.iter().any(|&byte| byte > 0x7f) {
		return Err(NameError::Encoding);
	}
	for &byte in bytes {
		match byte {
			0..=31 | 127 | b'<' | b'>' | b':' | b'"' | b'/' | b'\\' | b'|' | b'?' | b'*' => {
				return Err(NameError::ForbiddenCharacter);
			}
			_ => {}
		}
	}
	if matches!(bytes.first(), Some(b' ')) || matches!(bytes.last(), Some(b' ' | b'.')) {
		return Err(NameError::TrimmedName);
	}
	if reserved_device(bytes) {
		return Err(NameError::DeviceName);
	}
	Ok(())
}

fn reserved_device(bytes: &[u8]) -> bool {
	let mut stem = [0; COMPONENT_BYTES];
	let mut significant = 0;
	for (index, &byte) in bytes.iter().enumerate() {
		if byte == b'.' {
			break;
		}
		stem[index] = match byte {
			b'a'..=b'z' => byte - (b'a' - b'A'),
			_ => byte,
		};
		if byte != b' ' {
			significant = index + 1;
		}
	}
	match &stem[..significant] {
		b"CON" | b"PRN" | b"AUX" | b"NUL" | b"CONIN$" | b"CONOUT$" => true,
		[b'C', b'O', b'M', b'0'..=b'9'] | [b'L', b'P', b'T', b'0'..=b'9'] => true,
		_ => false,
	}
}

#[kani::proof]
#[kani::unwind(16)]
fn posix_component_bytes_match_relative_grammar() {
	let bytes: [u8; COMPONENT_BYTES] = kani::any();
	let len: usize = kani::any();
	kani::assume(len <= COMPONENT_BYTES);
	let max_bytes: usize = kani::any();
	let name = &bytes[..len];
	let expected = posix_component(name, max_bytes);
	assert_eq!(
		validate_component(name, NameProfile::PosixUtf8, max_bytes),
		expected
	);
	kani::cover!(expected.is_ok() && name == b"\xc3\xa9");
	kani::cover!(expected == Err(NameError::NotComponent) && name == b"..");
	kani::cover!(expected == Err(NameError::NotComponent) && name == b"a/b");
	kani::cover!(expected == Err(NameError::NotComponent) && name == b"a\0b");
	kani::cover!(expected == Err(NameError::TooLong));
}

#[kani::proof]
#[kani::unwind(16)]
fn portable_component_bytes_match_relative_grammar() {
	let bytes: [u8; COMPONENT_BYTES] = kani::any();
	let len: usize = kani::any();
	kani::assume(len <= COMPONENT_BYTES);
	let max_bytes: usize = kani::any();
	let name = &bytes[..len];
	let expected = portable_component(name, max_bytes);
	assert_eq!(
		validate_component(name, NameProfile::PortableAscii, max_bytes),
		expected
	);
	kani::cover!(expected.is_ok() && name == b"README.txt");
	kani::cover!(expected == Err(NameError::DeviceName) && name == b"lPt9 .x");
	kani::cover!(expected == Err(NameError::DeviceName) && name == b"CONOUT$.x");
	kani::cover!(expected == Err(NameError::ForbiddenCharacter) && name == b"a\\b");
	kani::cover!(expected == Err(NameError::ForbiddenCharacter) && name == b"a:b");
	kani::cover!(expected == Err(NameError::TrimmedName));
	kani::cover!(expected == Err(NameError::Encoding));
}

#[kani::proof]
fn extraction_path_step_matches_widened_length() {
	let (parent_bytes, has_parent, name_bytes, limit): (usize, bool, usize, usize) = kani::any();
	let end = parent_bytes as u128 + u128::from(has_parent) + name_bytes as u128;
	let actual = charge_child_path::<Infallible>(parent_bytes, has_parent, name_bytes, limit);
	assert_eq!(actual.is_ok(), end <= limit as u128);
	if let Ok(actual) = actual {
		assert_eq!(actual as u128, end);
	} else {
		assert!(matches!(
			actual,
			Err(NamePlanError::Limit(NameBudget::RelativePathBytes))
		));
	}
	kani::cover!(end > usize::MAX as u128);
	kani::cover!(has_parent && name_bytes != 0 && end == limit as u128);
}

#[kani::proof]
#[kani::unwind(10)]
fn extraction_path_prefixes_match_eight_component_budget() {
	let widths: [u8; COMPONENTS] = kani::any();
	let count: usize = kani::any();
	kani::assume(count <= COMPONENTS);
	let limit: usize = kani::any();
	let mut names = 0_u128;
	let mut prefix = 0;
	for (index, &width) in widths[..count].iter().enumerate() {
		let width = usize::from(width);
		kani::assume((1..=COMPONENT_BYTES).contains(&width));
		names += width as u128;
		let expected = names + index as u128;
		let actual = charge_child_path::<Infallible>(prefix, index != 0, width, limit);
		assert_eq!(actual.is_ok(), expected <= limit as u128);
		match actual {
			Ok(bytes) => {
				assert_eq!(bytes as u128, expected);
				prefix = bytes;
			}
			Err(NamePlanError::Limit(NameBudget::RelativePathBytes)) => return,
			_ => unreachable!("relative path charge returns only the relative path error"),
		}
	}
	kani::cover!(count == 0 && prefix == 0);
	kani::cover!(count == COMPONENTS);
}
