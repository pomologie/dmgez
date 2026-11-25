use std::process::ExitCode;

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod project;
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod server;
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod transport;

#[cfg(any(target_os = "linux", target_os = "macos"))]
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

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn main() -> ExitCode {
	eprintln!("dmgez-mcp requires macOS or Linux for descriptor-based project confinement");
	ExitCode::FAILURE
}
