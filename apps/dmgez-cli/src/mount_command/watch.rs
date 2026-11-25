use std::{
	io::{self, Write},
	time::Duration,
};

use dmgez_mount::watch::Selection;
use dmgez_runtime::mount_watch::{self, Eject, Request, Status};
use serde::Serialize;

use crate::{CliResult, report_output::write_report};

use super::WatchArguments;

const SCHEMA_VERSION: u8 = 1;
const PAIR_COUNT: &str = "supply one destination for each --watch-install application";

pub(super) fn run(arguments: &WatchArguments, json: bool) -> CliResult {
	let report = mount_watch::run(
		Request {
			id: &arguments.id,
			applications: &arguments.applications,
			destinations: &arguments.destination,
			selection: if arguments.match_any {
				Selection::Any
			} else {
				Selection::All
			},
			ejection: if arguments.auto_eject {
				Eject::Detach
			} else {
				Eject::KeepMounted
			},
			timeout: Duration::from_secs(arguments.timeout),
		},
		|id| {
			writeln!(
				io::stderr().lock(),
				"Watching fresh application copies for session {id}"
			)
		},
	)
	.map_err(|error| -> Box<dyn std::error::Error> {
		if matches!(error, mount_watch::Error::PairCount) {
			return PAIR_COUNT.into();
		}
		error.into()
	})?;
	render(&arguments.id, report.status, report.matched, json)
}

#[derive(Serialize)]
struct Report<'a> {
	schema_version: u8,
	session_id: &'a str,
	status: &'static str,
	matched: usize,
}

fn render(id: &str, status: Status, matched: usize, json: bool) -> CliResult {
	let report = Report {
		schema_version: SCHEMA_VERSION,
		session_id: id,
		status: status.as_str(),
		matched,
	};
	write_report(json, &report, |output| {
		writeln!(output, "session_id = {}", report.session_id)?;
		writeln!(output, "status = {status:?}")?;
		writeln!(output, "matched = {matched}")?;
		Ok(())
	})?;
	if status.succeeded() {
		Ok(())
	} else {
		Err(format!("install watch ended with {status:?}").into())
	}
}
