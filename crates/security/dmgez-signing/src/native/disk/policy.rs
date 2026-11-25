use std::path::Path;

use crate::{
	SignError, Status, is_valid_disk_identifier,
	native::policy::{AppIdentity, display_fields, has_timestamp},
};

use super::{Artifact, inspect};

pub(super) struct DeveloperId<'a> {
	identity: AppIdentity,
	identifier: &'a str,
}

impl<'a> DeveloperId<'a> {
	pub(super) fn new(fingerprint: &str, identifier: &'a str) -> Result<Self, SignError> {
		let identity = AppIdentity::new(fingerprint)?;
		if !is_valid_disk_identifier(identifier) {
			return Err(SignError::Identifier);
		}
		Ok(Self {
			identity,
			identifier,
		})
	}

	pub(super) fn fingerprint(&self) -> &str {
		self.identity.fingerprint()
	}

	pub(super) fn sign(&self, artifact: &Path) -> Result<(), SignError> {
		self.identity.sign_disk(artifact, self.identifier)
	}

	pub(super) fn verify(&self, artifact: &Artifact) -> Result<(), SignError> {
		artifact.validate()?;
		let result =
			self.identity
				.verify_requirement(artifact.path(), Some(self.identifier), false);
		artifact.validate()?;
		result?;
		let inspected = inspect(artifact)?;
		if inspected.status != Status::Present {
			return Err(SignError::SigningPolicy);
		}
		if !inspect_timestamp(
			artifact.path(),
			inspected.diagnostics.as_deref().unwrap_or_default(),
		) {
			return Err(SignError::Timestamp);
		}
		Ok(())
	}
}

fn inspect_timestamp(path: &Path, output: &str) -> bool {
	let path = path.to_string_lossy();
	display_fields(Path::new(path.as_ref()), output.as_bytes()).is_some_and(has_timestamp)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn only_the_native_secure_timestamp_field_is_admitted() {
		let path = Path::new("/private/tmp/Stage\nTimestamp=fake.dmg");
		let header = format!("Executable={}\nFormat=disk image\n", path.display());
		assert!(!inspect_timestamp(
			path,
			&format!("{header}Signed Time=Nov 25, 2026\n")
		));
		assert!(inspect_timestamp(
			path,
			&format!("{header}Timestamp=Nov 25, 2026\n")
		));
		assert!(!inspect_timestamp(
			path,
			&format!("{header}Timestamp=Nov 25, 2026\nTimestamp=ambiguous\n"),
		));
	}
}
