use std::{ffi::OsStr, process::Command, time::Duration};

use dmgez_core::process::run;
pub(super) use dmgez_core::process::{Error, Output};

use crate::NativeAssessmentTool;

const CODESIGN: &str = "/usr/bin/codesign";
const DITTO: &str = "/usr/bin/ditto";
const HDIUTIL: &str = "/usr/bin/hdiutil";
#[cfg(feature = "notarization-native")]
const LIST: &str = "/bin/ls";
#[cfg(feature = "notarization-native")]
const ACL_LIST_OPTIONS: &str = "-ldben";
const SECURITY: &str = "/usr/bin/security";
const SPCTL: &str = "/usr/sbin/spctl";
const SYSPOLICY_CHECK: &str = "/usr/bin/syspolicy_check";
#[cfg(feature = "notarization-native")]
const XCRUN: &str = "/usr/bin/xcrun";
#[cfg(feature = "notarization-native")]
const NOTARYTOOL: &str = "notarytool";
#[cfg(feature = "notarization-native")]
const STAPLER: &str = "stapler";
#[cfg(feature = "notarization-native")]
const FIND_TOOL: &str = "--find";
#[cfg(feature = "notarization-native")]
const TOOL_LOOKUP_TIMEOUT: Duration = Duration::from_secs(10);
const IDENTITY_LOOKUP: &[&str] = &["find-identity", "-v", "-p", "codesigning"];
const TIMEOUT: Duration = Duration::from_secs(120);
const MAX_OUTPUT_BYTES: usize = 256 * 1024;

pub(super) fn codesign(args: &[&OsStr]) -> Result<Output, Error> {
	run(Command::new(CODESIGN).args(args), TIMEOUT, MAX_OUTPUT_BYTES)
}

pub(super) fn ditto(args: &[&OsStr]) -> Result<Output, Error> {
	run(Command::new(DITTO).args(args), TIMEOUT, MAX_OUTPUT_BYTES)
}

pub(super) fn hdiutil(args: &[&OsStr]) -> Result<Output, Error> {
	run(Command::new(HDIUTIL).args(args), TIMEOUT, MAX_OUTPUT_BYTES)
}

pub(super) fn assess(tool: NativeAssessmentTool, args: &[&OsStr]) -> Result<Output, Error> {
	let path = match tool {
		NativeAssessmentTool::Spctl => SPCTL,
		NativeAssessmentTool::SyspolicyCheck => SYSPOLICY_CHECK,
	};
	run(Command::new(path).args(args), TIMEOUT, MAX_OUTPUT_BYTES)
}

#[cfg(feature = "notarization-native")]
pub(super) fn list_acl(path: &std::path::Path) -> Result<Output, Error> {
	run(
		Command::new(LIST).arg(ACL_LIST_OPTIONS).arg(path),
		TIMEOUT,
		MAX_OUTPUT_BYTES,
	)
}

pub(super) fn find_code_signing_identities() -> Result<Output, Error> {
	run(
		Command::new(SECURITY).args(IDENTITY_LOOKUP),
		TIMEOUT,
		MAX_OUTPUT_BYTES,
	)
}

#[cfg(feature = "notarization-native")]
pub(super) fn notarytool(
	args: &[&OsStr],
	timeout: Duration,
	output_limit: usize,
) -> Result<Output, Error> {
	run(
		Command::new(XCRUN).arg(NOTARYTOOL).args(args),
		timeout,
		output_limit,
	)
}

#[cfg(feature = "notarization-native")]
pub(super) fn find_stapler() -> Result<Output, Error> {
	run(
		Command::new(XCRUN).args([FIND_TOOL, STAPLER]),
		TOOL_LOOKUP_TIMEOUT,
		MAX_OUTPUT_BYTES,
	)
}

#[cfg(feature = "notarization-native")]
pub(super) fn stapler(args: &[&OsStr]) -> Result<Output, Error> {
	run(
		Command::new(XCRUN).arg(STAPLER).args(args),
		TIMEOUT,
		MAX_OUTPUT_BYTES,
	)
}
