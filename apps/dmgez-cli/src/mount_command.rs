use std::path::PathBuf;

use clap::{Args, Subcommand, ValueEnum};

use crate::CliResult;

#[cfg(all(feature = "mount-system", target_os = "macos"))]
mod native;

pub(super) const FEATURE: &str = "mount-system";
pub(super) const COMPILED: bool = cfg!(all(feature = "mount-system", target_os = "macos"));

const SESSION_OPTION_CONFLICT: &str =
	"mount session commands cannot take an image, mountpoint, backend or detach flag";
const IMAGE_REQUIRED: &str = "mount requires an image path or a session command";

#[derive(Args)]
pub(super) struct Arguments {
	/// Local disk image to attach read-only.
	image: Option<PathBuf>,
	/// Existing empty directory to use in place of a private session directory.
	#[arg(long)]
	mountpoint: Option<PathBuf>,
	/// Host mount backend.
	#[arg(long, value_enum)]
	backend: Option<Backend>,
	/// Return the session ID immediately and leave the image attached.
	#[arg(long)]
	detach: bool,
	/// Write a versioned JSON session report.
	#[arg(long, global = true)]
	json: bool,
	#[command(subcommand)]
	action: Option<Action>,
}

#[derive(Subcommand)]
enum Action {
	/// List dmgez's recorded sessions.
	List,
	/// Observe a single recorded session.
	Status { id: String },
	/// Request a non-forced detach of a recorded session.
	Stop { id: String },
}

#[derive(Args)]
pub(super) struct UnmountArguments {
	/// Session ID returned by dmgez mount.
	id: String,
	/// Write a versioned JSON session report.
	#[arg(long)]
	json: bool,
}

#[derive(Clone, Copy, ValueEnum)]
enum Backend {
	Auto,
	System,
	Fuse,
	Winfsp,
}

pub(super) fn run(arguments: &Arguments) -> CliResult {
	if arguments.action.is_some()
		&& (arguments.image.is_some()
			|| arguments.mountpoint.is_some()
			|| arguments.backend.is_some()
			|| arguments.detach)
	{
		return Err(SESSION_OPTION_CONFLICT.into());
	}
	if arguments.action.is_none() && arguments.image.is_none() {
		return Err(IMAGE_REQUIRED.into());
	}
	let admitted = require_backend(arguments.backend.unwrap_or(Backend::Auto));
	#[cfg(all(feature = "mount-system", target_os = "macos"))]
	{
		admitted?;
		native::run(arguments)
	}
	#[cfg(not(all(feature = "mount-system", target_os = "macos")))]
	admitted
}

pub(super) fn unmount(arguments: &UnmountArguments) -> CliResult {
	let admitted = require_backend(Backend::System);
	#[cfg(all(feature = "mount-system", target_os = "macos"))]
	{
		admitted?;
		native::stop(&arguments.id, arguments.json)
	}
	#[cfg(not(all(feature = "mount-system", target_os = "macos")))]
	{
		let _ = arguments;
		admitted
	}
}

fn require_backend(backend: Backend) -> CliResult {
	let unavailable = match backend {
		Backend::Auto | Backend::System if COMPILED => return Ok(()),
		Backend::Auto | Backend::System
			if !cfg!(feature = "mount-system") && cfg!(target_os = "macos") =>
		{
			return Err(format!("feature not compiled: {FEATURE}").into());
		}
		Backend::Auto | Backend::System => "system",
		Backend::Fuse => "fuse",
		Backend::Winfsp => "winfsp",
	};
	Err(format!("read-only mount backend unavailable: {unavailable}").into())
}
