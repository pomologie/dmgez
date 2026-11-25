use std::{path::PathBuf, process::ExitCode};

use clap::{Args, Subcommand};

use crate::{COMPILED, CliResult, NOT_COMPILED};

#[cfg(all(feature = "notarization-native", target_os = "macos"))]
mod report;

pub(super) const NATIVE_FEATURE: &str = "notarization-native";
const STATUS_ID_ARG: &str = "id";
const RECORD_ARG: &str = "record";
pub(super) const NATIVE_CAPABILITY: &str = match (
	cfg!(feature = "notarization-native"),
	cfg!(target_os = "macos"),
) {
	(false, _) => NOT_COMPILED,
	(true, false) => "unavailable",
	(true, true) => COMPILED,
};

#[derive(Args)]
#[command(args_conflicts_with_subcommands = true, subcommand_negates_reqs = true)]
pub(super) struct Arguments {
	/// Signed disk image to submit.
	#[arg(required = true, value_name = "IMAGE")]
	image: Option<PathBuf>,
	/// Create a private submission record at a new path.
	#[arg(long, required = true, value_name = "PATH")]
	record: Option<PathBuf>,
	#[command(flatten)]
	options: Options,
	#[command(subcommand)]
	action: Option<Action>,
}

#[derive(Subcommand)]
enum Action {
	/// Query a submission by ID or use the ID from its private record.
	Status {
		/// Apple notarization submission UUID. Defaults to the bound record ID.
		#[arg(value_name = "ID", required_unless_present = RECORD_ARG)]
		id: Option<String>,
		/// Update an existing private submission record.
		#[arg(long, value_name = "PATH", required_unless_present = STATUS_ID_ARG)]
		record: Option<PathBuf>,
		#[command(flatten)]
		options: Options,
	},
}

#[derive(Args)]
struct Options {
	/// Name of an existing notarization profile in macOS Keychain.
	#[arg(long, required = true, value_name = "NAME")]
	keychain_profile: Option<String>,
	/// Poll the submission until it finishes or the local wait expires.
	///
	/// Apple continues processing after the local wait ends.
	#[arg(long)]
	wait: bool,
	/// Write a versioned JSON report.
	#[arg(long)]
	json: bool,
}

#[cfg(not(feature = "notarization-native"))]
pub(super) fn run(_arguments: &Arguments) -> CliResult<ExitCode> {
	Err(format!("feature not compiled: {NATIVE_FEATURE}").into())
}

#[cfg(all(feature = "notarization-native", not(target_os = "macos")))]
pub(super) fn run(_arguments: &Arguments) -> CliResult<ExitCode> {
	Err("native notarization is unavailable on this platform".into())
}

#[cfg(all(feature = "notarization-native", target_os = "macos"))]
pub(super) fn run(arguments: &Arguments) -> CliResult<ExitCode> {
	use dmgez_signing::notarization::{Status, status, status_from_record, submit};

	use crate::REPORT_ERROR_EXIT_CODE;

	const REJECTED_EXIT_CODE: u8 = 1;

	let (result, json) = match &arguments.action {
		Some(Action::Status {
			id,
			record,
			options,
		}) => (
			if let Some(id) = id {
				status(id, record.as_deref(), options.profile()?, options.wait)?
			} else {
				let record = record
					.as_deref()
					.ok_or("--record is required when no ID is supplied")?;
				status_from_record(record, options.profile()?, options.wait)?
			},
			options.json,
		),
		None => {
			let image = arguments.image.as_deref().ok_or("an image is required")?;
			let record = arguments.record.as_deref().ok_or("--record is required")?;
			let options = &arguments.options;
			(
				submit(image, record, options.profile()?, options.wait)?,
				options.json,
			)
		}
	};
	report::write(&result, json)?;
	Ok(match result.status {
		Status::Pending | Status::Accepted => ExitCode::SUCCESS,
		Status::Rejected => ExitCode::from(REJECTED_EXIT_CODE),
		_ => ExitCode::from(REPORT_ERROR_EXIT_CODE),
	})
}

#[cfg(all(feature = "notarization-native", target_os = "macos"))]
impl Options {
	fn profile(&self) -> CliResult<&str> {
		self.keychain_profile
			.as_deref()
			.ok_or_else(|| "--keychain-profile is required".into())
	}
}
