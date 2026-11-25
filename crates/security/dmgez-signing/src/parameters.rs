const SHA1_HEX_BYTES: usize = 40;
const MAX_DISK_IDENTIFIER_BYTES: usize = 255;

/// Checks the syntax of a public SHA-1 certificate fingerprint used by the
/// native signer. Identity lookup and certificate trust are separate checks.
pub fn is_valid_certificate_fingerprint(fingerprint: &str) -> bool {
	fingerprint.len() == SHA1_HEX_BYTES && fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Checks whether a code-signing identifier is accepted by the disk signer.
pub fn is_valid_disk_identifier(identifier: &str) -> bool {
	identifier.len() <= MAX_DISK_IDENTIFIER_BYTES
		&& identifier
			.as_bytes()
			.first()
			.is_some_and(u8::is_ascii_alphanumeric)
		&& identifier
			.bytes()
			.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

/// Checks the syntax of an opaque Keychain profile name. This does not access
/// Keychain or disclose whether the profile exists.
pub fn is_valid_keychain_profile_name(profile: &str) -> bool {
	!profile.trim().is_empty() && !profile.chars().any(char::is_control)
}
