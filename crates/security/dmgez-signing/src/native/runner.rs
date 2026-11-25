use std::{
	collections::TryReserveError,
	ffi::OsStr,
	io::{self, Read},
	os::fd::AsFd,
	process::{Child, Command, ExitStatus, Stdio},
	thread,
	time::{Duration, Instant},
};

use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};

#[cfg(test)]
mod tests;

const CODESIGN: &str = "/usr/bin/codesign";
const SYSTEM_ROOT: &str = "/";
const SYSTEM_PATH: &str = "/usr/bin:/bin";
const LOCALE: &str = "C";
const TIMEOUT: Duration = Duration::from_secs(120);
const MAX_OUTPUT_BYTES: usize = 256 * 1024;
const READ_CHUNK_BYTES: usize = 8 * 1024;
const IDLE_POLL: Duration = Duration::from_millis(10);

pub(super) struct Output {
	pub(super) status: ExitStatus,
	pub(super) stdout: Vec<u8>,
	pub(super) stderr: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
pub(super) enum Error {
	#[error("could not start codesign")]
	Spawn(#[source] io::Error),
	#[error("could not collect codesign output or exit status")]
	Capture(#[source] io::Error),
	#[error("could not allocate the bounded codesign output buffer")]
	Allocation(#[from] TryReserveError),
	#[error("codesign did not finish within {timeout:?}")]
	Timeout { timeout: Duration },
	#[error("codesign exceeded its combined output limit of {limit} bytes")]
	OutputLimit { limit: usize },
	#[error("could not clean up codesign after {operation}")]
	Cleanup {
		operation: Box<Error>,
		#[source]
		source: io::Error,
	},
}

pub(super) fn codesign(args: &[&OsStr]) -> Result<Output, Error> {
	run(Command::new(CODESIGN).args(args), TIMEOUT, MAX_OUTPUT_BYTES)
}

fn run(command: &mut Command, timeout: Duration, output_limit: usize) -> Result<Output, Error> {
	let started = Instant::now();
	let mut child = command
		.env_clear()
		.env("LC_ALL", LOCALE)
		.env("PATH", SYSTEM_PATH)
		.current_dir(SYSTEM_ROOT)
		.stdin(Stdio::null())
		.stdout(Stdio::piped())
		.stderr(Stdio::piped())
		.spawn()
		.map_err(Error::Spawn)?;
	// Collection always closes both of its read pipes before cleanup.
	match collect(&mut child, started, timeout, output_limit) {
		Ok(output) => Ok(output),
		Err(operation) => match kill_and_reap(&mut child) {
			Ok(()) => Err(operation),
			Err(source) => Err(Error::Cleanup {
				operation: Box::new(operation),
				source,
			}),
		},
	}
}

fn collect(
	child: &mut Child,
	started: Instant,
	timeout: Duration,
	output_limit: usize,
) -> Result<Output, Error> {
	let (stdout, stderr) = (child.stdout.take(), child.stderr.take());
	let mut stdout = Some(nonblocking_pipe(stdout)?);
	let mut stderr = Some(nonblocking_pipe(stderr)?);
	let mut stdout_bytes = Vec::new();
	let mut stderr_bytes = Vec::new();
	let mut budget = OutputBudget {
		remaining: output_limit,
		limit: output_limit,
	};
	loop {
		let remaining_time = timeout.saturating_sub(started.elapsed());
		if remaining_time.is_zero() {
			return Err(Error::Timeout { timeout });
		}
		// Read at most one chunk per pipe each iteration to service both streams
		// and keep checking the deadline even when the child writes continuously.
		let stdout_progress = read_chunk(&mut stdout, &mut stdout_bytes, &mut budget)?;
		let stderr_progress = read_chunk(&mut stderr, &mut stderr_bytes, &mut budget)?;
		if stdout.is_none()
			&& stderr.is_none()
			&& let Some(status) = child.try_wait().map_err(Error::Capture)?
		{
			return Ok(Output {
				status,
				stdout: stdout_bytes,
				stderr: stderr_bytes,
			});
		}
		if !(stdout_progress || stderr_progress) {
			thread::sleep(remaining_time.min(IDLE_POLL));
		}
	}
}

fn nonblocking_pipe<Fd: AsFd>(pipe: Option<Fd>) -> Result<Fd, Error> {
	let pipe = pipe.ok_or_else(|| Error::Capture(io::Error::other("missing output pipe")))?;
	let flags = fcntl_getfl(&pipe).map_err(|error| Error::Capture(error.into()))?;
	fcntl_setfl(&pipe, flags | OFlags::NONBLOCK).map_err(|error| Error::Capture(error.into()))?;
	Ok(pipe)
}

struct OutputBudget {
	remaining: usize,
	limit: usize,
}

fn read_chunk(
	pipe: &mut Option<impl Read>,
	output: &mut Vec<u8>,
	budget: &mut OutputBudget,
) -> Result<bool, Error> {
	let Some(reader) = pipe else {
		return Ok(false);
	};
	let mut buffer = [0u8; READ_CHUNK_BYTES];
	// One extra byte detects output beyond an exactly exhausted budget.
	let read_limit = budget.remaining.saturating_add(1).min(buffer.len());
	match reader.read(&mut buffer[..read_limit]) {
		Ok(0) => {
			*pipe = None;
			Ok(true)
		}
		Ok(count) => {
			budget.remaining = budget
				.remaining
				.checked_sub(count)
				.ok_or(Error::OutputLimit {
					limit: budget.limit,
				})?;
			output.try_reserve(count)?;
			output.extend_from_slice(&buffer[..count]);
			Ok(true)
		}
		Err(error)
			if matches!(
				error.kind(),
				io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
			) =>
		{
			Ok(false)
		}
		Err(error) => Err(Error::Capture(error)),
	}
}

fn kill_and_reap(child: &mut Child) -> io::Result<()> {
	if let Err(error) = child.kill()
		&& child.try_wait()?.is_none()
	{
		return Err(error);
	}
	child.wait()?;
	Ok(())
}
