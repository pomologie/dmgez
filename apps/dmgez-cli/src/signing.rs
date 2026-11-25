use std::{path::PathBuf, process::ExitCode};

use clap::{Args, Subcommand, ValueEnum};

use crate::{COMPILED, CliResult, NOT_COMPILED};

#[cfg(feature = "signing-native")]
mod report;

pub(super) const NATIVE_FEATURE: &str = "signing-native";
pub(super) const NATIVE_CAPABILITY: &str =
	match (cfg!(feature = "signing-native"), cfg!(target_os = "macos")) {
		(false, _) => NOT_COMPILED,
		(true, false) => "unavailable",
		(true, true) => COMPILED,
	};

#[derive(Subcommand)]
pub(super) enum Action {
	/// Display code-signature information for an existing artifact.
	Inspect(Arguments),
	/// Verify the code signature on an existing artifact.
	Verify(Arguments),
}

#[derive(Args)]
pub(super) struct Arguments {
	/// Path to an application bundle or disk image.
	artifact: PathBuf,
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

#[cfg(not(feature = "signing-native"))]
pub(super) fn run(_action: &Action) -> CliResult<ExitCode> {
	Err(format!("feature not compiled: {NATIVE_FEATURE}").into())
}

#[cfg(feature = "signing-native")]
pub(super) fn run(action: &Action) -> CliResult<ExitCode> {
	use dmgez_signing::{Backend, Operation, Status, check_signature};

	use crate::REPORT_ERROR_EXIT_CODE;

	const FAILED_CHECK_EXIT_CODE: u8 = 1;

	let (operation, arguments) = match action {
		Action::Inspect(arguments) => (Operation::Inspect, arguments),
		Action::Verify(arguments) => (Operation::Verify, arguments),
	};
	let backend = match arguments.backend {
		BackendChoice::Auto => Backend::Auto,
		BackendChoice::Native => Backend::Native,
		BackendChoice::Rust => Backend::Rust,
	};
	let result = check_signature(&arguments.artifact, operation, backend)?;
	report::write(&result, arguments.json)?;
	Ok(match result.status {
		Status::Present | Status::Verified => ExitCode::SUCCESS,
		Status::Unsigned | Status::Failed => ExitCode::from(FAILED_CHECK_EXIT_CODE),
		_ => ExitCode::from(REPORT_ERROR_EXIT_CODE),
	})
}
