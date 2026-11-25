use std::{ffi::OsStr, path::Path};

use crate::{Error, SignError};

use super::{
	END_OPTIONS, EXECUTABLE_PREFIX, FORMAT_PREFIX, INSPECT_OPTIONS, identity::valid_fingerprint,
	runner,
};

const SIGN: &str = "--sign";
const TIMESTAMP_OPTION: &str = "--timestamp";
const IDENTIFIER_OPTION: &str = "--identifier";
const FORCE: &str = "--force";
const OPTIONS: &str = "--options";
const RUNTIME: &str = "runtime";
const ENTITLEMENTS: &str = "--entitlements";
const VERIFY_OPTIONS: &[&str] = &["--verify", "--strict", "--all-architectures"];
const REQUIREMENT: &str = "--test-requirement";
const DEEP: &str = "--deep";
const ARCHITECTURE: &str = "--architecture";
const DEVELOPER_ID_POLICY: &str = concat!(
	"=anchor apple generic",
	" and certificate 1[field.1.2.840.113635.100.6.2.6] exists",
	" and certificate leaf[field.1.2.840.113635.100.6.1.13] exists",
);
const TIMESTAMP_PREFIX: &str = "Timestamp=";
const DIRECTORY_PREFIX: &str = "CodeDirectory ";
const FLAGS_PREFIX: &str = "flags=0x";
const HARDENED_RUNTIME_FLAG: u32 = 0x0001_0000;

pub(super) struct AppIdentity {
	fingerprint: String,
}

impl AppIdentity {
	pub(super) fn new(fingerprint: &str) -> Result<Self, SignError> {
		if !valid_fingerprint(fingerprint) {
			return Err(SignError::Fingerprint);
		}
		Ok(Self {
			fingerprint: fingerprint.to_ascii_uppercase(),
		})
	}

	pub(super) fn fingerprint(&self) -> &str {
		&self.fingerprint
	}

	pub(super) fn sign(
		&self,
		path: &Path,
		runtime: bool,
		entitlements: Option<&Path>,
	) -> Result<(), SignError> {
		let mut arguments = self.signing_arguments();
		arguments.push(OsStr::new(FORCE));
		if runtime {
			arguments.extend([OsStr::new(OPTIONS), OsStr::new(RUNTIME)]);
		}
		if let Some(entitlements) = entitlements {
			arguments.extend([OsStr::new(ENTITLEMENTS), entitlements.as_os_str()]);
		}
		sign_path(path, arguments)
	}

	pub(super) fn verify(
		&self,
		path: &Path,
		executable: &Path,
		architectures: &[&str],
		runtime: bool,
		deep: bool,
	) -> Result<(), SignError> {
		self.verify_requirement(path, None, deep)?;
		let architectures = architectures
			.iter()
			.copied()
			.map(Some)
			.chain(architectures.is_empty().then_some(None));
		for architecture in architectures {
			let display = inspect(path, architecture)?;
			let fields =
				display_fields(executable, &display.stderr).ok_or(SignError::SigningPolicy)?;
			verify_display(fields, runtime)?;
		}
		Ok(())
	}

	pub(in crate::native) fn sign_disk(
		&self,
		path: &Path,
		identifier: &str,
	) -> Result<(), SignError> {
		let mut arguments = self.signing_arguments();
		arguments.extend([OsStr::new(IDENTIFIER_OPTION), OsStr::new(identifier)]);
		sign_path(path, arguments)
	}

	pub(in crate::native) fn verify_requirement(
		&self,
		path: &Path,
		identifier: Option<&str>,
		deep: bool,
	) -> Result<(), SignError> {
		let requirement = format!(
			"{DEVELOPER_ID_POLICY} and certificate leaf = H\"{}\"",
			self.fingerprint,
		);
		let requirement = match identifier {
			Some(identifier) => format!("{requirement} and identifier \"{identifier}\""),
			None => requirement,
		};
		verify_requirement(path, &requirement, deep)
	}

	fn signing_arguments(&self) -> Vec<&OsStr> {
		vec![
			OsStr::new(SIGN),
			OsStr::new(&self.fingerprint),
			OsStr::new(TIMESTAMP_OPTION),
		]
	}
}

#[cfg(feature = "notarization-native")]
pub(super) fn verify_notarization_image(path: &Path) -> Result<(), SignError> {
	let display = inspect(path, None)?;
	if !super::is_disk_image(path, &display.stderr) {
		return Err(SignError::SigningPolicy);
	}
	let fields = display_fields(path, &display.stderr).ok_or(SignError::SigningPolicy)?;
	verify_display(fields, false)?;
	verify_requirement(path, DEVELOPER_ID_POLICY, false)
}

fn verify_requirement(path: &Path, requirement: &str, deep: bool) -> Result<(), SignError> {
	let mut arguments: Vec<_> = VERIFY_OPTIONS.iter().map(OsStr::new).collect();
	if deep {
		arguments.push(OsStr::new(DEEP));
	}
	arguments.extend([
		OsStr::new(REQUIREMENT),
		OsStr::new(requirement),
		OsStr::new(END_OPTIONS),
		path.as_os_str(),
	]);
	let result = codesign(&arguments)?;
	if result.status.success() {
		Ok(())
	} else {
		Err(SignError::SigningPolicy)
	}
}

pub(in crate::native) fn display_fields<'a>(
	executable: &Path,
	output: &'a [u8],
) -> Option<&'a str> {
	let fields = output
		.strip_prefix(EXECUTABLE_PREFIX)?
		.strip_prefix(executable.as_os_str().as_encoded_bytes())?
		.strip_prefix(b"\n")?;
	std::str::from_utf8(fields).ok()
}

pub(in crate::native) fn has_timestamp(fields: &str) -> bool {
	let Some((format_at, _)) = unique_field(fields, format_prefix()) else {
		return false;
	};
	let Some((timestamp_at, timestamp)) = unique_field(fields, TIMESTAMP_PREFIX) else {
		return false;
	};
	!timestamp.trim().is_empty() && timestamp_at > format_at
}

fn format_prefix() -> &'static str {
	std::str::from_utf8(FORMAT_PREFIX).expect("native field prefix is ASCII")
}

fn verify_display(fields: &str, runtime: bool) -> Result<(), SignError> {
	if !has_timestamp(fields) {
		return Err(SignError::Timestamp);
	}
	if runtime && !has_hardened_runtime(fields) {
		return Err(SignError::HardenedRuntime);
	}
	Ok(())
}

fn inspect(path: &Path, architecture: Option<&str>) -> Result<runner::Output, SignError> {
	let mut arguments: Vec<_> = INSPECT_OPTIONS.iter().map(OsStr::new).collect();
	if let Some(architecture) = architecture {
		arguments.extend([OsStr::new(ARCHITECTURE), OsStr::new(architecture)]);
	}
	arguments.extend([OsStr::new(END_OPTIONS), path.as_os_str()]);
	let output = codesign(&arguments)?;
	if output.status.success() {
		Ok(output)
	} else {
		Err(SignError::SigningPolicy)
	}
}

fn unique_field<'a>(fields: &'a str, prefix: &str) -> Option<(usize, &'a str)> {
	let mut values = fields
		.lines()
		.enumerate()
		.filter_map(|(index, line)| line.strip_prefix(prefix).map(|value| (index, value)));
	let value = values.next()?;
	values.next().is_none().then_some(value)
}

pub(in crate::native) fn has_hardened_runtime(fields: &str) -> bool {
	let Some((_, directory)) = unique_field(fields, DIRECTORY_PREFIX) else {
		return false;
	};
	directory
		.split_ascii_whitespace()
		.find_map(|part| part.strip_prefix(FLAGS_PREFIX))
		.and_then(|flags| flags.split_once('('))
		.and_then(|(bits, _)| u32::from_str_radix(bits, 16).ok())
		.is_some_and(|flags| flags & HARDENED_RUNTIME_FLAG != 0)
}

fn sign_path<'a>(path: &'a Path, mut arguments: Vec<&'a OsStr>) -> Result<(), SignError> {
	arguments.extend([OsStr::new(END_OPTIONS), path.as_os_str()]);
	let result = codesign(&arguments)?;
	if result.status.success() {
		Ok(())
	} else {
		Err(SignError::SigningFailed {
			status: result.status,
		})
	}
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
	fn native_runtime_proof_requires_the_flag_in_one_code_directory() {
		const PREFIX: &str = "CodeDirectory v=20500 size=344 ";
		assert!(has_hardened_runtime(&format!(
			"{PREFIX}flags=0x10000(runtime) hashes=5+2"
		)));
		assert!(has_hardened_runtime(&format!(
			"{PREFIX}flags=0x10200(kill,runtime) hashes=5+2"
		)));
		assert!(!has_hardened_runtime(&format!(
			"{PREFIX}flags=0x200(kill) hashes=5+2"
		)));
		assert!(!has_hardened_runtime(&format!(
			"{PREFIX}flags=0x10000(runtime)\n{PREFIX}flags=0x10000(runtime)"
		)));
	}

	#[test]
	fn identifier_text_cannot_supply_timestamp_evidence() {
		let fields = concat!(
			"Identifier=com.example.app\n",
			"Timestamp=untrusted identifier continuation\n",
			"Format=app bundle with Mach-O thin (arm64)\n",
			"CodeDirectory v=20500 size=344 flags=0x10000(runtime)\n",
		);
		assert!(!has_timestamp(fields));
	}
}
