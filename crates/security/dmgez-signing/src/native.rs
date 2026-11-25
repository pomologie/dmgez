use std::{ffi::OsStr, io, path::Path};

use crate::{
	ArtifactKind, ArtifactProblem, Backend, Error, Operation, Report, ResolvedBackend, Status,
	signature::report_base,
};

pub(super) mod app;
mod artifact;
pub(super) mod disk;
mod identity;
mod policy;
mod runner;
mod stage;

#[cfg(test)]
mod tests;

use artifact::Artifact;
use runner::Output;

const INSPECT_OPTIONS: &[&str] = &["--display", "--verbose=4"];
const BUNDLE_VERIFY_OPTIONS: &[&str] = &[
	"--verify",
	"--strict",
	"--all-architectures",
	"--deep",
	"--verbose=2",
];
const IMAGE_VERIFY_OPTIONS: &[&str] = &["--verify", "--strict", "--verbose=2"];
const END_OPTIONS: &str = "--";
const SIGNATURE_OPERATION_FAILURE: i32 = 1;
const UNSIGNED_MESSAGE: &[u8] = b": code object is not signed at all\n";
const ARCHITECTURE_PREFIX: &[u8] = b"In architecture: ";
const EXECUTABLE_PREFIX: &[u8] = b"Executable=";
const FORMAT_PREFIX: &[u8] = b"Format=";
const DISK_IMAGE_FORMAT: &[u8] = b"disk image";

pub(super) fn check(path: &Path, operation: Operation, backend: Backend) -> Result<Report, Error> {
	let artifact = Artifact::open(path)?;
	let mut report = report_base(artifact.path(), operation, backend);
	report.kind = Some(artifact.kind());
	report.backend = Some(ResolvedBackend::Native);

	if artifact.kind() == ArtifactKind::DiskImage && operation == Operation::Verify {
		// codesign also accepts arbitrary signed files. Confirm which format it
		// recognized before interpreting a successful check as a DMG result.
		run(&artifact, Operation::Inspect, &mut report)?;
		if report.status != Status::Present {
			return Ok(report);
		}
	}
	run(&artifact, operation, &mut report)?;
	Ok(report)
}

fn run(artifact: &Artifact, operation: Operation, report: &mut Report) -> Result<(), Error> {
	artifact.validate()?;
	let options = match (operation, artifact.kind()) {
		(Operation::Inspect, _) => INSPECT_OPTIONS,
		(Operation::Verify, ArtifactKind::AppBundle) => BUNDLE_VERIFY_OPTIONS,
		(Operation::Verify, ArtifactKind::DiskImage) => IMAGE_VERIFY_OPTIONS,
	};
	let mut arguments: Vec<_> = options.iter().map(OsStr::new).collect();
	arguments.extend([OsStr::new(END_OPTIONS), artifact.path().as_os_str()]);
	let result = runner::codesign(&arguments);
	artifact.validate()?;
	let output = match result {
		Ok(output) => output,
		Err(runner::Error::Spawn(error)) if error.kind() == io::ErrorKind::NotFound => {
			report.status = Status::Unavailable;
			report.diagnostics = Some("system codesign tool is unavailable".to_owned());
			return Ok(());
		}
		Err(source) => {
			return Err(Error::Tool {
				source: Box::new(source),
			});
		}
	};

	report.status = match output.status.code() {
		Some(0) if operation == Operation::Inspect => {
			if artifact.kind() == ArtifactKind::DiskImage
				&& !is_disk_image(artifact.path(), &output.stderr)
			{
				return Err(artifact.invalid(ArtifactProblem::NativeFormat));
			}
			Status::Present
		}
		Some(0) => Status::Verified,
		Some(SIGNATURE_OPERATION_FAILURE) if is_unsigned(artifact.path(), &output) => {
			Status::Unsigned
		}
		Some(SIGNATURE_OPERATION_FAILURE) => Status::Failed,
		_ => {
			return Err(Error::ToolExit {
				status: output.status,
			});
		}
	};
	report.diagnostics = diagnostic_text(output);
	Ok(())
}

fn is_disk_image(path: &Path, stderr: &[u8]) -> bool {
	// Exclude the echoed operand, which can contain line breaks and field names.
	let Some(fields) = stderr
		.strip_prefix(EXECUTABLE_PREFIX)
		.and_then(|tail| tail.strip_prefix(path.as_os_str().as_encoded_bytes()))
		.and_then(|tail| tail.strip_prefix(b"\n"))
	else {
		return false;
	};
	let mut formats = fields
		.split(|&byte| byte == b'\n')
		.filter_map(|line| line.strip_prefix(FORMAT_PREFIX));
	formats.next() == Some(DISK_IMAGE_FORMAT) && formats.next().is_none()
}

fn is_unsigned(path: &Path, output: &Output) -> bool {
	let stderr = output.stderr.as_slice();
	let Some(suffix) = stderr
		.strip_prefix(path.as_os_str().as_encoded_bytes())
		.and_then(|tail| tail.strip_prefix(UNSIGNED_MESSAGE))
	else {
		return false;
	};
	output.stdout.is_empty() && (suffix.is_empty() || is_architecture_line(suffix))
}

fn is_architecture_line(suffix: &[u8]) -> bool {
	suffix
		.strip_prefix(ARCHITECTURE_PREFIX)
		.and_then(|tail| tail.strip_suffix(b"\n"))
		.is_some_and(|name| {
			!name.is_empty()
				&& name
					.iter()
					.all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
		})
}

fn diagnostic_text(output: Output) -> Option<String> {
	let stdout = String::from_utf8_lossy(&output.stdout);
	let stderr = String::from_utf8_lossy(&output.stderr);
	match (stdout.is_empty(), stderr.is_empty()) {
		(true, true) => None,
		(false, true) => Some(stdout.into_owned()),
		(true, false) => Some(stderr.into_owned()),
		(false, false) => Some(format!("{stdout}\n{stderr}")),
	}
}
