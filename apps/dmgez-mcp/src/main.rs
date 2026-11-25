use std::process::ExitCode;

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
mod image;
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
mod local;
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
mod project;
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
mod server;
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
mod transport;

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
fn main() -> ExitCode {
	use clap::Parser;

	let options = server::Options::parse();
	match server::run(options) {
		Ok(()) => ExitCode::SUCCESS,
		Err(error) => {
			eprintln!("dmgez-mcp: {error}");
			ExitCode::FAILURE
		}
	}
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn main() -> ExitCode {
	eprintln!("dmgez-mcp requires macOS, Linux, or Windows for handle-based file confinement");
	ExitCode::FAILURE
}
