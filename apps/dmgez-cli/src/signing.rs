use std::{ffi::OsString, path::PathBuf, process::ExitCode};

use clap::{Args, Subcommand, ValueEnum};

use crate::{COMPILED, CliResult, NOT_COMPILED};

#[cfg(any(feature = "signing-native", feature = "signing-rust"))]
mod report;

pub(super) const NATIVE_FEATURE: &str = "signing-native";
pub(super) const NATIVE_CAPABILITY: &str =
	match (cfg!(feature = "signing-native"), cfg!(target_os = "macos")) {
		(false, _) => NOT_COMPILED,
		(true, false) => "unavailable",
		(true, true) => COMPILED,
	};
pub(super) const RUST_FEATURE: &str = "signing-rust";
pub(super) const RUST_CAPABILITY: &str = match (
	cfg!(feature = "signing-rust"),
	cfg!(any(target_os = "linux", target_os = "macos", windows)),
) {
	(false, _) => NOT_COMPILED,
	(true, false) => "unavailable",
	(true, true) => COMPILED,
};

const AD_HOC_ARG: &str = "ad_hoc";
const IDENTITY_ARG: &str = "identity";

#[derive(Subcommand)]
pub(super) enum Action {
	/// Display code-signature information for an existing artifact.
	Inspect(Arguments),
	/// Verify the code signature on an existing artifact.
	Verify(Arguments),
	/// Sign an unsigned disk image and publish a verified copy.
	Dmg(DiskSigningArguments),
	/// Sign an application bundle and publish a verified copy.
	App(AppSigningArguments),
}

#[derive(Args)]
pub(super) struct Arguments {
	/// Path to an application bundle or disk image.
	artifact: PathBuf,
	#[command(flatten)]
	options: SigningOptions,
}

#[derive(Args)]
pub(super) struct DiskSigningArguments {
	/// Unsigned disk image to copy and sign.
	source: PathBuf,
	/// Publish the signed image to a new path.
	#[arg(long, value_name = "PATH")]
	output: PathBuf,
	/// SHA-1 fingerprint of a Developer ID Application certificate in the keychain.
	#[arg(long, id = IDENTITY_ARG, value_name = "SHA1", required_unless_present = AD_HOC_ARG)]
	identity: Option<String>,
	/// Sign without a certificate. The default backend is apple-codesign.
	#[arg(long, id = AD_HOC_ARG, conflicts_with = IDENTITY_ARG)]
	ad_hoc: bool,
	/// Code signing identifier for the disk image.
	#[arg(long, value_name = "ID")]
	identifier: String,
	#[command(flatten)]
	options: SigningOptions,
}

#[derive(Args)]
pub(super) struct AppSigningArguments {
	/// Application bundle to copy and sign.
	source: PathBuf,
	/// Publish the signed application to a new path.
	#[arg(long, value_name = "PATH")]
	output: PathBuf,
	/// SHA-1 fingerprint of a Developer ID Application certificate in the keychain.
	#[arg(long, value_name = "SHA1")]
	identity: String,
	/// Entitlements for an exact app-relative code path. Use "." for the root bundle. May be repeated.
	#[arg(long, value_name = "CODE_PATH=PLIST")]
	entitlements: Vec<OsString>,
	#[command(flatten)]
	options: SigningOptions,
}

#[derive(Args)]
struct SigningOptions {
	/// Select the backend for this operation.
	#[arg(long, value_enum, default_value_t)]
	backend: BackendChoice,
	/// Write a versioned JSON report.
	#[arg(long)]
	json: bool,
}

#[derive(Clone, Copy, Default, ValueEnum)]
enum BackendChoice {
	#[default]
	Auto,
	Native,
	Rust,
}

#[cfg(any(feature = "signing-native", feature = "signing-rust"))]
impl From<BackendChoice> for dmgez_signing::Backend {
	fn from(choice: BackendChoice) -> Self {
		match choice {
			BackendChoice::Auto => Self::Auto,
			BackendChoice::Native => Self::Native,
			BackendChoice::Rust => Self::Rust,
		}
	}
}

pub(super) fn run(action: &Action) -> CliResult<ExitCode> {
	match action {
		#[cfg(any(feature = "signing-native", feature = "signing-rust"))]
		Action::Inspect(arguments) => check(dmgez_signing::Operation::Inspect, arguments),
		#[cfg(any(feature = "signing-native", feature = "signing-rust"))]
		Action::Verify(arguments) => check(dmgez_signing::Operation::Verify, arguments),
		#[cfg(not(any(feature = "signing-native", feature = "signing-rust")))]
		Action::Inspect(_) | Action::Verify(_) => {
			Err(format!("feature not compiled: {NATIVE_FEATURE} or {RUST_FEATURE}").into())
		}
		Action::App(arguments) => sign_app(arguments),
		Action::Dmg(arguments) if arguments.ad_hoc => sign_ad_hoc(arguments),
		Action::Dmg(arguments) => sign_developer_id(arguments),
	}
}

#[cfg(feature = "signing-rust")]
fn sign_ad_hoc(arguments: &DiskSigningArguments) -> CliResult<ExitCode> {
	let result = dmgez_signing::sign_ad_hoc_disk_image(
		&arguments.source,
		&arguments.output,
		&arguments.identifier,
		arguments.options.backend.into(),
	)?;
	report::write_disk_signing(&result, arguments.options.json)?;
	Ok(ExitCode::SUCCESS)
}

#[cfg(not(feature = "signing-rust"))]
fn sign_ad_hoc(_: &DiskSigningArguments) -> CliResult<ExitCode> {
	Err(format!("feature not compiled: {RUST_FEATURE}").into())
}

#[cfg(feature = "signing-native")]
fn sign_developer_id(arguments: &DiskSigningArguments) -> CliResult<ExitCode> {
	let identity = arguments
		.identity
		.as_deref()
		.ok_or("missing signing identity")?;
	let result = dmgez_signing::sign_disk_image(
		&arguments.source,
		&arguments.output,
		identity,
		&arguments.identifier,
		arguments.options.backend.into(),
	)?;
	report::write_disk_signing(&result, arguments.options.json)?;
	Ok(ExitCode::SUCCESS)
}

#[cfg(not(feature = "signing-native"))]
fn sign_developer_id(_: &DiskSigningArguments) -> CliResult<ExitCode> {
	Err(format!("feature not compiled: {NATIVE_FEATURE}").into())
}

#[cfg(not(feature = "signing-native"))]
fn sign_app(_: &AppSigningArguments) -> CliResult<ExitCode> {
	Err(format!("feature not compiled: {NATIVE_FEATURE}").into())
}

#[cfg(feature = "signing-native")]
fn sign_app(arguments: &AppSigningArguments) -> CliResult<ExitCode> {
	use dmgez_signing::sign_application;

	let entitlements = arguments
		.entitlements
		.iter()
		.map(|mapping| entitlement_mapping(mapping))
		.collect::<CliResult<Vec<_>>>()?;
	let result = sign_application(
		&arguments.source,
		&arguments.output,
		&arguments.identity,
		&entitlements,
		arguments.options.backend.into(),
	)?;
	report::write_app_signing(&result, arguments.options.json)?;
	Ok(ExitCode::SUCCESS)
}

#[cfg(feature = "signing-native")]
fn entitlement_mapping(value: &std::ffi::OsStr) -> CliResult<dmgez_signing::AppEntitlements<'_>> {
	use std::{ffi::OsStr, path::Path};

	const SEPARATOR: char = '=';
	const EXPECTED_MAPPING: &str =
		"--entitlements requires CODE_PATH=PLIST with both paths present";

	#[cfg(unix)]
	let parts = {
		use std::os::unix::ffi::OsStrExt;

		let bytes = value.as_bytes();
		bytes
			.iter()
			.position(|&byte| byte == SEPARATOR as u8)
			.map(|delimiter| {
				(
					OsStr::from_bytes(&bytes[..delimiter]),
					OsStr::from_bytes(&bytes[delimiter + 1..]),
				)
			})
	};
	#[cfg(not(unix))]
	let parts = value
		.to_str()
		.and_then(|value| value.split_once(SEPARATOR))
		.map(|(item, plist)| (OsStr::new(item), OsStr::new(plist)));
	let (item, plist) = parts
		.filter(|(item, plist)| !item.is_empty() && !plist.is_empty())
		.ok_or(EXPECTED_MAPPING)?;
	Ok(dmgez_signing::AppEntitlements {
		item: Path::new(item),
		plist: Path::new(plist),
	})
}

#[cfg(any(feature = "signing-native", feature = "signing-rust"))]
fn check(operation: dmgez_signing::Operation, arguments: &Arguments) -> CliResult<ExitCode> {
	use dmgez_signing::{Status, check_signature};

	use crate::REPORT_ERROR_EXIT_CODE;

	const FAILED_CHECK_EXIT_CODE: u8 = 1;

	let result = check_signature(
		&arguments.artifact,
		operation,
		arguments.options.backend.into(),
	)?;
	report::write(&result, arguments.options.json)?;
	Ok(match result.status {
		Status::Present | Status::Verified => ExitCode::SUCCESS,
		Status::Unsigned | Status::Failed => ExitCode::from(FAILED_CHECK_EXIT_CODE),
		_ => ExitCode::from(REPORT_ERROR_EXIT_CODE),
	})
}
