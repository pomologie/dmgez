use std::{ffi::OsStr, path::Path};

use crate::{Error, SignError, Status};

use super::{Artifact, inspect};
use crate::native::{END_OPTIONS, EXECUTABLE_PREFIX, identity::valid_fingerprint, runner};

const MAX_IDENTIFIER_BYTES: usize = 255;
const SIGN_OPTIONS: &[&str] = &["--sign"];
const TIMESTAMP_OPTION: &str = "--timestamp";
const IDENTIFIER_OPTION: &str = "--identifier";
const VERIFY_OPTIONS: &[&str] = &["--verify", "--strict", "--test-requirement"];
const DEVELOPER_ID_POLICY: &str = concat!(
	"=anchor apple generic",
	" and certificate 1[field.1.2.840.113635.100.6.2.6] exists",
	" and certificate leaf[field.1.2.840.113635.100.6.1.13] exists",
);
const TIMESTAMP_PREFIX: &str = "Timestamp=";

pub(super) struct DeveloperId<'a> {
	fingerprint: String,
	identifier: &'a str,
}

impl<'a> DeveloperId<'a> {
	pub(super) fn new(fingerprint: &str, identifier: &'a str) -> Result<Self, SignError> {
		if !valid_fingerprint(fingerprint) {
			return Err(SignError::Fingerprint);
		}
		if identifier.len() > MAX_IDENTIFIER_BYTES
			|| !identifier
				.as_bytes()
				.first()
				.is_some_and(u8::is_ascii_alphanumeric)
			|| !identifier
				.bytes()
				.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
		{
			return Err(SignError::Identifier);
		}
		Ok(Self {
			fingerprint: fingerprint.to_ascii_uppercase(),
			identifier,
		})
	}

	pub(super) fn fingerprint(&self) -> &str {
		&self.fingerprint
	}

	pub(super) fn sign(&self, artifact: &Path) -> Result<(), SignError> {
		let mut arguments: Vec<_> = SIGN_OPTIONS.iter().map(OsStr::new).collect();
		arguments.extend([
			OsStr::new(&self.fingerprint),
			OsStr::new(TIMESTAMP_OPTION),
			OsStr::new(IDENTIFIER_OPTION),
			OsStr::new(self.identifier),
			OsStr::new(END_OPTIONS),
			artifact.as_os_str(),
		]);
		let result = codesign(&arguments)?;
		if result.status.success() {
			Ok(())
		} else {
			Err(SignError::SigningFailed {
				status: result.status,
			})
		}
	}

	pub(super) fn verify(&self, artifact: &Artifact) -> Result<(), SignError> {
		let requirement = self.requirement();
		let mut arguments: Vec<_> = VERIFY_OPTIONS.iter().map(OsStr::new).collect();
		arguments.extend([
			OsStr::new(&requirement),
			OsStr::new(END_OPTIONS),
			artifact.path().as_os_str(),
		]);
		artifact.validate()?;
		let result = codesign(&arguments)?;
		artifact.validate()?;
		if !result.status.success() {
			return Err(SignError::SigningPolicy);
		}
		let inspected = inspect(artifact)?;
		if inspected.status != Status::Present {
			return Err(SignError::SigningPolicy);
		}
		if !has_timestamp(
			artifact.path(),
			inspected.diagnostics.as_deref().unwrap_or_default(),
		) {
			return Err(SignError::Timestamp);
		}
		Ok(())
	}

	fn requirement(&self) -> String {
		format!(
			"{DEVELOPER_ID_POLICY} and certificate leaf = H\"{}\" and identifier \"{}\"",
			self.fingerprint, self.identifier,
		)
	}
}

fn has_timestamp(path: &Path, output: &str) -> bool {
	let path = path.to_string_lossy();
	let Some(fields) = output
		.as_bytes()
		.strip_prefix(EXECUTABLE_PREFIX)
		.and_then(|tail| tail.strip_prefix(path.as_bytes()))
		.and_then(|tail| tail.strip_prefix(b"\n"))
	else {
		return false;
	};
	let Ok(fields) = std::str::from_utf8(fields) else {
		return false;
	};
	let mut timestamps = fields
		.lines()
		.filter_map(|line| line.strip_prefix(TIMESTAMP_PREFIX));
	timestamps
		.next()
		.is_some_and(|value| !value.trim().is_empty())
		&& timestamps.next().is_none()
}

fn codesign(arguments: &[&OsStr]) -> Result<runner::Output, SignError> {
	runner::codesign(arguments).map_err(|source| {
		Error::Tool {
			source: Box::new(source),
		}
		.into()
	})
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn only_the_native_secure_timestamp_field_is_admitted() {
		let path = Path::new("/private/tmp/Stage\nTimestamp=fake.dmg");
		let header = format!("Executable={}\nFormat=disk image\n", path.display());
		assert!(!has_timestamp(
			path,
			&format!("{header}Signed Time=Nov 25, 2026\n")
		));
		assert!(has_timestamp(
			path,
			&format!("{header}Timestamp=Nov 25, 2026\n")
		));
		assert!(!has_timestamp(
			path,
			&format!("{header}Timestamp=Nov 25, 2026\nTimestamp=ambiguous\n"),
		));
	}
}
