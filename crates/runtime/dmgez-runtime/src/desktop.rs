//! Process handoff to a separately installed desktop executable.

use std::{
	env, fs, io,
	path::{self, Path, PathBuf},
	process::{Child, Command, Stdio},
};

use thiserror::Error;

const GUI_NAME: &str = "dmgez-gui";
#[cfg(windows)]
const EXECUTABLE_EXTENSION: &str = "exe";

/// Why the local desktop handoff could not be started.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum LaunchError {
	#[error("could not resolve the calling executable")]
	CurrentExecutable(#[source] io::Error),
	#[error("could not access the requested document {path:?}")]
	Document {
		path: PathBuf,
		#[source]
		source: io::Error,
	},
	#[error("the requested document is not a regular file at {0:?}")]
	DocumentType(PathBuf),
	#[error(
		"desktop executable unavailable at {path:?}; install the desktop beside dmgez or supply --gui"
	)]
	Unavailable {
		path: PathBuf,
		#[source]
		source: io::Error,
	},
	#[error("the configured desktop program is not a supported executable file at {0:?}")]
	ExecutableType(PathBuf),
	#[error("could not start the desktop executable at {path:?}")]
	Start {
		path: PathBuf,
		#[source]
		source: io::Error,
	},
}

/// Starts the configured GUI, or the desktop executable beside the caller.
///
/// The document is passed as one native path argument. The GUI owns admission
/// and displays document errors. The returned handle lets callers observe or
/// reap the child. An exiting command-line caller may drop the handle.
///
/// # Errors
/// Returns a typed error if the document or program cannot be resolved, a file
/// has the wrong type, or the operating system rejects process creation.
pub fn launch(document: &Path, configured: Option<&Path>) -> Result<Child, LaunchError> {
	let program = match configured {
		Some(path) => path.to_owned(),
		None => env::current_exe()
			.map_err(LaunchError::CurrentExecutable)?
			.with_file_name(format!("{GUI_NAME}{}", env::consts::EXE_SUFFIX)),
	};
	let executable = fs::canonicalize(&program).map_err(|source| LaunchError::Unavailable {
		path: program.clone(),
		source,
	})?;
	let metadata = fs::metadata(&executable).map_err(|source| LaunchError::Unavailable {
		path: program,
		source,
	})?;
	if !metadata.is_file() {
		return Err(LaunchError::ExecutableType(executable));
	}
	#[cfg(windows)]
	if !executable
		.extension()
		.is_some_and(|ext| ext.eq_ignore_ascii_case(EXECUTABLE_EXTENSION))
	{
		return Err(LaunchError::ExecutableType(executable));
	}

	let absolute = path::absolute(document).map_err(|source| LaunchError::Document {
		path: document.to_owned(),
		source,
	})?;
	let metadata = fs::metadata(&absolute).map_err(|source| LaunchError::Document {
		path: absolute.clone(),
		source,
	})?;
	if !metadata.is_file() {
		return Err(LaunchError::DocumentType(absolute));
	}
	Command::new(&executable)
		.arg(absolute)
		.stdin(Stdio::null())
		.stdout(Stdio::null())
		.stderr(Stdio::null())
		.spawn()
		.map_err(|source| LaunchError::Start {
			path: executable,
			source,
		})
}
