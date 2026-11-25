use std::num::NonZeroU64;

use serde::Serialize;

use super::ReleaseError;

const SHA256_HEX_BYTES: usize = 64;
const SUBMISSION_ID_BYTES: usize = 36;
const UUID_SEPARATORS: [usize; 4] = [8, 13, 18, 23];

/// Identity of the exact bytes a release stage read or produced.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ImageIdentity {
	sha256: String,
	bytes: NonZeroU64,
}

impl ImageIdentity {
	/// Records a nonempty image's SHA-256 in canonical hexadecimal form.
	///
	/// # Errors
	/// Returns an error for an empty image or invalid SHA-256 representation.
	pub fn new(sha256: &str, bytes: u64) -> Result<Self, ReleaseError> {
		let bytes = NonZeroU64::new(bytes).ok_or(ReleaseError::ImageIdentity)?;
		if !valid_digest(sha256) {
			return Err(ReleaseError::ImageIdentity);
		}
		Ok(Self {
			sha256: sha256.to_ascii_lowercase(),
			bytes,
		})
	}

	pub fn sha256(&self) -> &str {
		&self.sha256
	}

	pub const fn bytes(&self) -> u64 {
		self.bytes.get()
	}
}

pub(crate) fn valid_digest(value: &str) -> bool {
	value.len() == SHA256_HEX_BYTES && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub(crate) fn valid_submission_id(value: &str) -> bool {
	value.len() == SUBMISSION_ID_BYTES
		&& value.bytes().enumerate().all(|(index, byte)| {
			if UUID_SEPARATORS.contains(&index) {
				byte == b'-'
			} else {
				byte.is_ascii_hexdigit()
			}
		})
}
