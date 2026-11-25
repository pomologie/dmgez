use std::{
	env, fs,
	io::{self, IsTerminal, Read, Write},
	path::{Path, PathBuf},
};

use dmgez_mount::{
	session::{Phase, SessionId, SessionRecord, SessionStore},
	system::{self, Observation, Request},
};
use serde::Serialize;

use crate::{
	CliResult,
	report_output::{EscapedText, write_report},
};

use super::{Action, Arguments};

const SESSION_ROOT: &str = "dmgez-mount-sessions-v1";
const REPORT_VERSION: u32 = 1;
const TERMINAL_REQUIRED: &str =
	"foreground mounting needs a terminal. Use --detach to keep the recorded session attached";
const NO_ATTACHMENT: &str = "the session has no confirmed attachment to detach";
const IMAGE_PATH_KIND: &str = "image";
const MOUNTPOINT_PATH_KIND: &str = "mountpoint";

pub(super) fn run(arguments: &Arguments) -> CliResult {
	match &arguments.action {
		Some(Action::List) => {
			let sessions = open_store()?.list()?;
			report(arguments.json, Operation::List, &sessions, None)
		}
		Some(Action::Status { id }) => status(id.parse()?, arguments.json),
		Some(Action::Stop { id }) => stop(id, arguments.json),
		#[cfg(feature = "mount-watch")]
		Some(Action::Watch(watch)) => super::watch::run(watch, arguments.json),
		#[cfg(not(feature = "mount-watch"))]
		Some(Action::Watch(_)) => Err(format!("feature not compiled: {}", super::WATCH_FEATURE).into()),
		None => start(arguments),
	}
}

pub(super) fn open_store() -> CliResult<SessionStore> {
	let directory = env::temp_dir().canonicalize()?.join(SESSION_ROOT);
	Ok(SessionStore::open(&directory)?)
}

fn start(arguments: &Arguments) -> CliResult {
	let image = arguments.image.as_deref().ok_or(super::IMAGE_REQUIRED)?;
	if !arguments.detach && !io::stdin().is_terminal() {
		return Err(TERMINAL_REQUIRED.into());
	}
	let image = requested_path(image, IMAGE_PATH_KIND)?;
	let mountpoint = arguments
		.mountpoint
		.as_deref()
		.map(|path| requested_path(path, MOUNTPOINT_PATH_KIND))
		.transpose()?;
	let store = open_store()?;
	let mut session = store.start(&image, mountpoint.as_deref())?;
	let request = Request::new(session.image(), session.mountpoint());
	let attachment = match system::attach(&request) {
		Ok(attachment) => attachment,
		Err(error) => {
			let saved = if error.attachment_may_exist() {
				store.mark_uncertain(&mut session)
			} else {
				store.mark_detached(&mut session)
			};
			writeln!(io::stderr().lock(), "session_id = {}", session.id())?;
			if let Err(record_error) = saved {
				let message = format!(
					"{error}. Could not record outcome for {} ({record_error})",
					session.id(),
				);
				return Err(message.into());
			}
			return Err(error.into());
		}
	};
	if let Err(error) = store.mark_mounted(&mut session, attachment.clone()) {
		writeln!(io::stderr().lock(), "session_id = {}", session.id())?;
		if let Err(detach) = system::detach(&attachment) {
			return Err(format!(
				"could not save the mount session ({error}). Detach failed ({detach})"
			)
			.into());
		}
		// A failed directory sync may leave the preceding record published.
		let saved = store.get(session.id()).and_then(|mut current| {
			if matches!(current.phase(), Phase::Detached) {
				return Ok(());
			}
			store.mark_detached(&mut current)
		});
		if let Err(record_error) = saved {
			return Err(format!(
				"{error}. Could not record the confirmed detach for {} ({record_error})",
				session.id()
			)
			.into());
		}
		return Err(error.into());
	}
	drop(store);
	if arguments.detach {
		return report(
			arguments.json,
			Operation::Mount,
			&[session],
			Some(Observation::Mounted),
		);
	}
	let id = session.id().to_string();
	writeln!(io::stderr().lock(), "session_id = {id}")?;
	writeln!(
		io::stderr().lock(),
		"Press Return to detach the read-only image."
	)?;
	let input = io::stdin().read(&mut [0]).map(|_| ());
	let detached = stop(&id, arguments.json);
	input?;
	detached
}

fn requested_path(path: &Path, kind: &str) -> CliResult<PathBuf> {
	if fs::symlink_metadata(path)?.is_symlink() {
		return Err(format!("{kind} cannot be a symbolic link: {path:?}").into());
	}
	Ok(path.canonicalize()?)
}

fn status(id: SessionId, json: bool) -> CliResult {
	let record = open_store()?.get(&id)?;
	let observed = if matches!(record.phase(), Phase::Detached) {
		Some(Observation::Detached)
	} else {
		record.last_attachment().map(system::observe).transpose()?
	};
	report(json, Operation::Status, &[record], observed)
}

pub(super) fn stop(id: &str, json: bool) -> CliResult {
	let id: SessionId = id.parse()?;
	let store = open_store()?;
	let mut record = store.get(&id)?;
	if !matches!(record.phase(), Phase::Detached) {
		let attachment = record.last_attachment().ok_or(NO_ATTACHMENT)?;
		if let Err(error) = system::detach(attachment) {
			if let Err(record_error) = store.mark_uncertain(&mut record) {
				let message =
					format!("{error}. Could not record outcome for {id} ({record_error})");
				return Err(message.into());
			}
			return Err(error.into());
		}
		store.mark_detached(&mut record)?;
	}
	report(
		json,
		Operation::Stop,
		&[record],
		Some(Observation::Detached),
	)
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum Operation {
	Mount,
	List,
	Status,
	Stop,
}

#[derive(Serialize)]
struct Report<'records> {
	schema_version: u32,
	operation: Operation,
	sessions: &'records [SessionRecord],
	#[serde(skip_serializing_if = "Option::is_none")]
	observed: Option<&'static str>,
}

fn report(
	json: bool,
	operation: Operation,
	sessions: &[SessionRecord],
	observed: Option<Observation>,
) -> CliResult {
	let observed = observed.map(|value| match value {
		Observation::Mounted => "mounted",
		Observation::Detached => "detached",
		Observation::IdentityMismatch => "identity_mismatch",
		_ => "unknown",
	});
	let report = Report {
		schema_version: REPORT_VERSION,
		operation,
		sessions,
		observed,
	};
	write_report(json, &report, |output| {
		for session in sessions {
			let phase = match session.phase() {
				Phase::Starting => "starting",
				Phase::Mounted(_) => "mounted",
				Phase::Uncertain => "uncertain",
				Phase::Detached => "detached",
				_ => "unknown",
			};
			writeln!(output, "session_id = {}", session.id())?;
			writeln!(output, "recorded_phase = {phase}")?;
			writeln!(
				output,
				"image = {}",
				EscapedText(&session.image().to_string_lossy())
			)?;
			writeln!(
				output,
				"mountpoint = {}",
				EscapedText(&session.mountpoint().to_string_lossy())
			)?;
		}
		if let Some(status) = observed {
			writeln!(output, "observed = {status}")?;
		}
		Ok(())
	})
}
