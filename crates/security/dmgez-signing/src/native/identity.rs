use std::process::ExitStatus;

use super::runner;
use crate::is_valid_certificate_fingerprint;

#[cfg(test)]
mod tests;

const VALID_IDENTITY_SUMMARY: &str = " valid identities found";
const DEVELOPER_ID_APPLICATION_PREFIX: &str = "Developer ID Application: ";

#[derive(Debug, thiserror::Error)]
pub(super) enum Error {
	#[error("a certificate fingerprint must contain exactly 40 hexadecimal digits")]
	Fingerprint,
	#[error("could not query the native code signing identities")]
	Lookup(#[source] runner::Error),
	#[error("native code signing identity lookup failed ({status})")]
	Exit { status: ExitStatus },
	#[error("could not interpret the native code signing identity listing")]
	MalformedOutput,
}

/// Checks the selected fingerprint and the label from Security's valid identities.
/// Cryptographic verification of the Apple chain is a separate post-signing check.
pub(super) fn developer_id_available(fingerprint: &str) -> Result<bool, Error> {
	if !is_valid_certificate_fingerprint(fingerprint) {
		return Err(Error::Fingerprint);
	}
	let output = runner::find_code_signing_identities().map_err(Error::Lookup)?;
	if !output.status.success() {
		return Err(Error::Exit {
			status: output.status,
		});
	}
	listed_developer_id(&output.stdout, fingerprint)
}

fn listed_developer_id(stdout: &[u8], fingerprint: &str) -> Result<bool, Error> {
	let mut lines = std::str::from_utf8(stdout)
		.map_err(|_| Error::MalformedOutput)?
		.lines();
	let count = lines
		.next_back()
		.and_then(|summary| {
			summary
				.trim_ascii_start()
				.strip_suffix(VALID_IDENTITY_SUMMARY)
		})
		.and_then(|number| number.parse::<usize>().ok())
		.ok_or(Error::MalformedOutput)?;
	let mut seen = 0;
	let mut found = false;
	for line in lines {
		seen += 1;
		let record = identity_record(line, seen).ok_or(Error::MalformedOutput)?;
		found |= record.fingerprint.eq_ignore_ascii_case(fingerprint)
			&& record.trust_annotation.is_empty()
			&& record
				.label
				.strip_prefix(DEVELOPER_ID_APPLICATION_PREFIX)
				.is_some_and(|name| !name.trim_ascii().is_empty());
	}
	if seen != count {
		return Err(Error::MalformedOutput);
	}
	Ok(found)
}

struct IdentityRecord<'output> {
	fingerprint: &'output str,
	label: &'output str,
	trust_annotation: &'output str,
}

fn identity_record(line: &str, expected_ordinal: usize) -> Option<IdentityRecord<'_>> {
	let (ordinal, entry) = line.trim_ascii_start().split_once(") ")?;
	if ordinal.parse::<usize>().ok()? != expected_ordinal {
		return None;
	}
	let (fingerprint, quoted_label) = entry.split_once(" \"")?;
	if !is_valid_certificate_fingerprint(fingerprint) {
		return None;
	}
	let (label, trust_annotation) = quoted_label.rsplit_once('"')?;
	if label.chars().any(char::is_control)
		|| !(trust_annotation.is_empty()
			|| trust_annotation.starts_with(" (") && trust_annotation.ends_with(')'))
	{
		return None;
	}
	Some(IdentityRecord {
		fingerprint,
		label,
		trust_annotation,
	})
}
