use std::{
	collections::HashSet,
	error::Error,
	fs::File,
	hash::Hash,
	io::{self, Write},
	path::absolute,
	thread,
	time::{Duration, Instant},
};

use dmgez_config::VirtualPath;
use dmgez_mount::{
	session::{Phase, SessionRecord, SessionStore},
	system::{self, Attachment, DetachOutcome},
	watch::{self, Action, Ejection, Monitor, Selection, Snapshot, StopReason},
};
use dmgez_operations::{
	extract::{NameLimits, NameProfile, host::StableDestination},
	install::{
		BundleIdentity, BundleStamp, CopyLimits, CopyPlan, ExistingBundleError, VerifiedBundle,
		observe_existing_bundle, prepare_application, prepare_copy,
		verify_existing_bundle_observed,
	},
};
use dmgez_vfs::volume::{VolumeBackend, VolumeView};
use serde::Serialize;

use crate::{CliResult, report_output::write_report, volume_input::with_hfs_file_verified};

use super::{WatchArguments, native::open_store};

const POLL_INTERVAL: Duration = Duration::from_millis(250);
const QUIET_TIME: Duration = Duration::from_secs(1);
const SCHEMA_VERSION: u8 = 1;
const SESSION_CHANGED: &str = "the selected mount session changed during the watch";
const ATTACHMENT_MISSING: &str = "install monitoring requires a confirmed read-only attachment";
const PAIR_COUNT: &str = "supply one destination for each --watch-install application";
const DESTINATION_NAME: &str = "the destination must end in the exact application basename";
const TARGET_SET: &str = "copy destinations must be distinct and outside the mounted image";

type TargetSnapshot = Snapshot<BundleIdentity, BundleStamp>;
type Observation = watch::Observation<TargetSnapshot>;

pub(super) fn run(arguments: &WatchArguments, json: bool) -> CliResult {
	let count = arguments.applications.len();
	if count == 0 || count > watch::MAX_TARGETS || count != arguments.destination.len() {
		return Err(PAIR_COUNT.into());
	}
	let started = Instant::now();
	let timeout = Duration::from_secs(arguments.timeout);
	let deadline = started
		.checked_add(timeout)
		.ok_or("watch deadline overflow")?;
	let store = open_store()?;
	let session = store.get(&arguments.id.parse()?)?;
	drop(store);
	let Phase::Mounted(attachment) = session.phase() else {
		return Err(ATTACHMENT_MISSING.into());
	};
	if system::observe(attachment)? != system::Observation::Mounted {
		return Err(ATTACHMENT_MISSING.into());
	}
	let file = system::open_attachment_source(attachment)?;
	let run = Run {
		session: &session,
		attachment,
		file: &file,
		deadline,
		auto_eject: arguments.auto_eject,
	};
	with_hfs_file_verified(&file, |view, verify_source| {
		let targets = prepare_targets(view, arguments, &run)?;
		let baseline = observations(&targets)?;
		verify_source()?;
		run.check_source()?;
		let selection = if arguments.match_any {
			Selection::Any
		} else {
			Selection::All
		};
		let mut monitor = Monitor::new(
			started,
			watch::Options {
				quiet: QUIET_TIME,
				timeout,
				selection,
			},
			&baseline,
		)?;
		writeln!(
			io::stderr().lock(),
			"Watching fresh application copies for session {}",
			session.id()
		)?;
		let (status, matched) = monitor_copies(&run, &mut monitor, &targets, view, verify_source)?;
		render(&session, status, matched, json)
	})
}

struct Run<'a> {
	session: &'a SessionRecord,
	attachment: &'a Attachment,
	file: &'a File,
	deadline: Instant,
	auto_eject: bool,
}

impl Run<'_> {
	fn expired(&self) -> bool {
		Instant::now() >= self.deadline
	}

	fn check_source(&self) -> io::Result<()> {
		system::check_attachment_source(self.attachment, self.file).map_err(io::Error::other)
	}

	fn current(&self, store: &SessionStore) -> CliResult<SessionRecord> {
		let current = store.get(self.session.id())?;
		if current.revision() != self.session.revision()
			|| !matches!(current.phase(), Phase::Mounted(_))
		{
			return Err(SESSION_CHANGED.into());
		}
		Ok(current)
	}
}

struct Target<Id> {
	plan: CopyPlan<Id>,
	directory: StableDestination,
}

fn prepare_targets<B: VolumeBackend>(
	view: &mut VolumeView<B>,
	arguments: &WatchArguments,
	run: &Run<'_>,
) -> CliResult<Vec<Target<B::Id>>>
where
	B::Id: Hash + Eq,
{
	let mut seen = HashSet::new();
	let mut targets = Vec::with_capacity(arguments.applications.len());
	for (application, target) in arguments.applications.iter().zip(&arguments.destination) {
		let target = absolute(target)?;
		if target.starts_with(run.attachment.mountpoint()) || !seen.insert(target.clone()) {
			return Err(TARGET_SET.into());
		}
		let source = application.parse::<VirtualPath>()?;
		let application = prepare_application(
			view,
			&source,
			NameProfile::PortableAscii,
			NameLimits::default(),
			|| run.expired(),
		)?;
		let name = std::ffi::OsStr::new(application.destination_name().as_str());
		if target.file_name() != Some(name) {
			return Err(DESTINATION_NAME.into());
		}
		let parent = target.parent().ok_or(DESTINATION_NAME)?;
		let directory = StableDestination::open(parent)?;
		let plan = prepare_copy(view, application, CopyLimits::default(), || run.expired())?;
		plan.require_host_metadata()?;
		targets.push(Target { plan, directory });
	}
	Ok(targets)
}

fn observations<Id>(targets: &[Target<Id>]) -> CliResult<Vec<Observation>> {
	targets
		.iter()
		.map(|target| {
			let directory = target.directory.directory();
			match observe_existing_bundle(&directory, target.plan.application().destination_name())
			{
				Ok(Some(stamp)) => Ok(Observation::Present(snapshot(stamp))),
				Ok(None) => Ok(Observation::Absent),
				Err(error) if retryable(&error) => Ok(Observation::Unavailable),
				Err(error) => Err(error.into()),
			}
		})
		.collect()
}

fn snapshot(stamp: BundleStamp) -> TargetSnapshot {
	Snapshot {
		identity: stamp.identity(),
		stamp,
	}
}

fn retryable<E: Error + 'static>(error: &ExistingBundleError<E>) -> bool {
	matches!(
		error,
		ExistingBundleError::Missing
			| ExistingBundleError::Mismatch
			| ExistingBundleError::Changed
			| ExistingBundleError::Dataless
			| ExistingBundleError::Cancelled
	)
}

fn monitor_copies<B: VolumeBackend>(
	run: &Run<'_>,
	monitor: &mut Monitor<BundleIdentity, BundleStamp>,
	targets: &[Target<B::Id>],
	view: &mut VolumeView<B>,
	verify_source: &dyn Fn() -> io::Result<()>,
) -> CliResult<(Status, usize)> {
	let mut proofs: Vec<Option<VerifiedBundle<'_, B::Id>>> =
		(0..targets.len()).map(|_| None).collect();
	loop {
		let observations = observations(targets)?;
		match monitor.observe(Instant::now(), &observations)? {
			Action::Wait => thread::sleep(
				POLL_INTERVAL.min(run.deadline.saturating_duration_since(Instant::now())),
			),
			Action::Verify(attempt) => {
				run.check_source()?;
				let index = attempt.target();
				let target = &targets[index];
				let proof = match verify_existing_bundle_observed(
					view,
					&target.plan,
					&target.directory.directory(),
					verify_source,
					|| run.expired(),
				) {
					Ok(proof) => Some(proof),
					Err(error) if retryable(&error) => None,
					Err(error) => return Err(error.into()),
				};
				let evidence = proof.as_ref().map(|proof| snapshot(proof.stamp()));
				monitor.verified(Instant::now(), attempt, evidence.as_ref())?;
				proofs[index] = proof;
			}
			Action::Recheck => {
				if let Some(result) = finish(run, monitor, targets, &proofs, verify_source)? {
					return Ok(result);
				}
			}
			Action::Attached(reason) => {
				return Ok((Status::from(reason), monitor.matched_targets().count()));
			}
			_ => return Err("the watch reached an invalid action".into()),
		}
	}
}

fn finish<Id>(
	run: &Run<'_>,
	monitor: &mut Monitor<BundleIdentity, BundleStamp>,
	targets: &[Target<Id>],
	proofs: &[Option<VerifiedBundle<'_, Id>>],
	verify_source: &dyn Fn() -> io::Result<()>,
) -> CliResult<Option<(Status, usize)>> {
	let store = open_store()?;
	let mut current = run.current(&store)?;
	let matched: Vec<_> = monitor.matched_targets().collect();
	let mut observed = observations(targets)?;
	for &index in &matched {
		if let Err(error) = recheck_target(run, &targets[index], proofs[index].as_ref()) {
			if !retryable(&error) {
				return Err(error.into());
			}
			observed[index] = Observation::Unavailable;
		}
	}
	match monitor.recheck(Instant::now(), &observed)? {
		Action::Wait => return Ok(None),
		Action::Attached(reason) => {
			return Ok(Some((
				Status::from(reason),
				monitor.matched_targets().count(),
			)));
		}
		Action::Eject => (),
		_ => return Err("the watch reached an invalid final action".into()),
	}
	let matched: Vec<_> = monitor.matched_targets().collect();
	let count = matched.len();
	let precondition = || {
		for index in matched {
			recheck_target(run, &targets[index], proofs[index].as_ref())
				.map_err(io::Error::other)?;
		}
		verify_source()?;
		run.check_source()?;
		if run.expired() {
			return Err(io::ErrorKind::TimedOut.into());
		}
		Ok(())
	};
	if !run.auto_eject {
		if system::observe(run.attachment)? != system::Observation::Mounted {
			return Err(SESSION_CHANGED.into());
		}
		precondition()?;
		return Ok(Some((Status::Verified, count)));
	}
	let (status, result) = match system::detach_after(run.attachment, precondition) {
		Ok(DetachOutcome::Detached) => (Status::Detached, Ejection::Detached),
		Ok(DetachOutcome::AlreadyDetached) => (Status::AlreadyDetached, Ejection::Detached),
		Err(system::Error::DetachBusy) => (Status::Busy, Ejection::Busy),
		Err(system::Error::DetachRefused { .. }) => (Status::Failed, Ejection::Failed),
		Err(error @ system::Error::DetachPrecondition { .. }) => return Err(error.into()),
		Err(error) => {
			store.mark_uncertain(&mut current)?;
			return Err(error.into());
		}
	};
	if result == Ejection::Detached {
		store.mark_detached(&mut current)?;
	}
	monitor.ejection_finished(Instant::now(), result)?;
	Ok(Some((status, count)))
}

fn recheck_target<Id>(
	run: &Run<'_>,
	target: &Target<Id>,
	proof: Option<&VerifiedBundle<'_, Id>>,
) -> Result<(), ExistingBundleError<std::convert::Infallible>> {
	proof.ok_or(ExistingBundleError::Changed)?.recheck(
		&target.plan,
		&target.directory.directory(),
		|| run.expired(),
	)
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum Status {
	Verified,
	Detached,
	AlreadyDetached,
	TimedOut,
	Cancelled,
	Busy,
	Failed,
}

impl From<StopReason> for Status {
	fn from(reason: StopReason) -> Self {
		match reason {
			StopReason::TimedOut => Self::TimedOut,
			StopReason::Cancelled => Self::Cancelled,
			StopReason::Busy => Self::Busy,
			_ => Self::Failed,
		}
	}
}

#[derive(Serialize)]
struct Report<'a> {
	schema_version: u8,
	session_id: &'a str,
	status: Status,
	matched: usize,
}

fn render(session: &SessionRecord, status: Status, matched: usize, json: bool) -> CliResult {
	let report = Report {
		schema_version: SCHEMA_VERSION,
		session_id: session.id().as_str(),
		status,
		matched,
	};
	write_report(json, &report, |output| {
		writeln!(output, "session_id = {}", report.session_id)?;
		writeln!(output, "status = {status:?}")?;
		writeln!(output, "matched = {matched}")?;
		Ok(())
	})?;
	match status {
		Status::Verified | Status::Detached | Status::AlreadyDetached => Ok(()),
		_ => Err(format!("install watch ended with {status:?}").into()),
	}
}
