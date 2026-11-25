use std::io::{self, IsTerminal, Read, Write};

use dmgez_mount::{
	session::{Phase, SessionRecord},
	system::Observation,
	workflow,
};
use serde::Serialize;

use crate::{
	CliResult,
	report_output::{EscapedText, write_report},
};

use super::{Action, Arguments};

const REPORT_VERSION: u32 = 1;
const TERMINAL_REQUIRED: &str =
	"foreground mounting needs a terminal. Use --detach to keep the recorded session attached";

pub(super) fn run(arguments: &Arguments) -> CliResult {
	match &arguments.action {
		Some(Action::List) => report(arguments.json, Operation::List, &workflow::list()?, None),
		Some(Action::Status { id }) => {
			let (record, observed) = workflow::status(&id.parse()?)?;
			report(arguments.json, Operation::Status, &[record], observed)
		}
		Some(Action::Stop { id }) => stop(id, arguments.json),
		#[cfg(feature = "mount-watch")]
		Some(Action::Watch(watch)) => super::watch::run(watch, arguments.json),
		#[cfg(not(feature = "mount-watch"))]
		Some(Action::Watch(_)) => Err(format!("feature not compiled: {}", super::WATCH_FEATURE).into()),
		None => start(arguments),
	}
}

fn start(arguments: &Arguments) -> CliResult {
	let image = arguments.image.as_deref().ok_or(super::IMAGE_REQUIRED)?;
	if !arguments.detach && !io::stdin().is_terminal() {
		return Err(TERMINAL_REQUIRED.into());
	}
	let session = match workflow::start(image, arguments.mountpoint.as_deref()) {
		Ok(session) => session,
		Err(error) => {
			if let Some(id) = error.failed_start_id() {
				writeln!(io::stderr().lock(), "session_id = {id}")?;
			}
			return Err(error.into());
		}
	};
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

pub(super) fn stop(id: &str, json: bool) -> CliResult {
	let record = workflow::stop(&id.parse()?)?;
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
