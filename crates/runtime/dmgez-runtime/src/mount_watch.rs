//! Bounded verification of fresh external copies from a recorded mount.

use std::{
	collections::HashSet,
	error::Error as StdError,
	fs::File,
	hash::Hash,
	io,
	path::{PathBuf, absolute},
	thread,
	time::{Duration, Instant},
};

use crate::volume::{self, with_hfs_file_verified};
use dmgez_config::VirtualPath;
use dmgez_mount::{
	session::{self, Phase, SessionId, SessionRecord, SessionStore},
	system::{self, Attachment, DetachOutcome},
	watch::{self, Action, Ejection, Monitor, Selection, Snapshot, StopReason},
	workflow,
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

const POLL_INTERVAL: Duration = Duration::from_millis(250);
const QUIET_TIME: Duration = Duration::from_secs(1);

type TargetSnapshot = Snapshot<BundleIdentity, BundleStamp>;
type Observation = watch::Observation<TargetSnapshot>;

/// Application paths and exact destination paths for one mount watch.
#[derive(Clone, Copy, Debug)]
pub struct Request<'a> {
	/// Recorded mount to observe.
	pub id: &'a str,
	/// Absolute paths to application bundles inside the image.
	pub applications: &'a [String],
	/// Exact local paths to watch, in the same order.
	pub destinations: &'a [PathBuf],
	/// Whether all copies or any copy must match.
	pub selection: Selection,
	/// Whether to detach the recorded mount after verifying the required copies.
	pub ejection: Eject,
	/// Deadline relative to the start of the operation, including admission.
	pub timeout: Duration,
}

/// What to do with the mount after successful verification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Eject {
	/// Leave the mount attached.
	KeepMounted,
	/// Request a normal, non-forced detach.
	Detach,
}

/// A failure admitting or running the watch.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
	/// Application and destination lists could not be paired within bounds.
	#[error("the number of applications and destinations must match within watch limits")]
	PairCount,
	/// The authorized copy destinations overlap or include the mounted image.
	#[error("copy destinations must be distinct and outside the mounted image")]
	TargetSet,
	/// The local target does not use the application's exact name.
	#[error("the destination must end in the exact application basename")]
	DestinationName,
	/// No matching live and read-only attachment could be confirmed.
	#[error("install monitoring requires a confirmed read-only attachment")]
	AttachmentMissing,
	/// The session record or native attachment no longer matches admission.
	#[error("the selected mount session changed during the watch")]
	SessionChanged,
	/// The timeout cannot be represented on the host clock.
	#[error("watch deadline overflow")]
	Deadline,
	/// An unexpected monitor action could not be applied.
	#[error("the watch reached an invalid action")]
	InvalidAction,
	/// An unexpected monitor action could not be applied before detaching.
	#[error("the watch reached an invalid final action")]
	InvalidFinalAction,
	/// The underlying watch state machine refused the transition.
	#[error(transparent)]
	Watch(#[from] watch::Error),
	/// Failure to inspect or operate on the native attachment.
	#[error(transparent)]
	System(#[from] system::Error),
	/// Failure to update the selected mount session.
	#[error(transparent)]
	Session(#[from] session::Error),
	/// Failure to open the session registry.
	#[error(transparent)]
	Registry(#[from] workflow::Error),
	/// Failure to validate or read the image container.
	#[error(transparent)]
	Image(#[from] dmgez_udif::Error),
	/// Failure to open the HFS+ volume.
	#[error(transparent)]
	Volume(#[from] volume::Error),
	/// Failure to read the host filesystem or report progress.
	#[error(transparent)]
	Io(#[from] io::Error),
	/// Failure to admit a target or verify an application copy.
	#[error(transparent)]
	Copy(Box<dyn StdError + Send + Sync>),
}

impl Error {
	fn copy(error: impl StdError + Send + Sync + 'static) -> Self {
		Self::Copy(Box::new(error))
	}
}

/// Completed watch outcome and the number of copies verified together.
#[derive(Clone, Copy, Debug)]
pub struct Report {
	/// Whether the copies were verified and what happened to the mount.
	pub status: Status,
	/// Number of matching copies at completion.
	pub matched: usize,
}

/// Checks explicit destinations against the held source and optionally detaches the mount.
///
/// The progress callback runs after admission and before the first poll.
///
/// # Errors
/// Returns the cause of admission, observation, verification, or session errors.
/// A successful call may report a timed-out watch or a refused detach.
pub fn run(
	request: Request<'_>,
	ready: impl FnOnce(&SessionId) -> io::Result<()>,
) -> Result<Report, Error> {
	let count = request.applications.len();
	if count == 0 || count > watch::MAX_TARGETS || count != request.destinations.len() {
		return Err(Error::PairCount);
	}
	let started = Instant::now();
	let deadline = started
		.checked_add(request.timeout)
		.ok_or(Error::Deadline)?;
	let store = workflow::open_store()?;
	let session = store.get(&request.id.parse()?)?;
	drop(store);
	let Phase::Mounted(attachment) = session.phase() else {
		return Err(Error::AttachmentMissing);
	};
	if system::observe(attachment)? != system::Observation::Mounted {
		return Err(Error::AttachmentMissing);
	}
	let file = system::open_attachment_source(attachment)?;
	let run = Run {
		session: &session,
		attachment,
		file: &file,
		deadline,
		ejection: request.ejection,
	};
	with_hfs_file_verified(&file, |view, verify_source| {
		let targets = prepare_targets(view, &request, &run)?;
		let baseline = observations(&targets)?;
		verify_source()?;
		run.check_source()?;
		let mut monitor = Monitor::new(
			started,
			watch::Options {
				quiet: QUIET_TIME,
				timeout: request.timeout,
				selection: request.selection,
			},
			&baseline,
		)?;
		ready(session.id())?;
		let (status, matched) = monitor_copies(&run, &mut monitor, &targets, view, verify_source)?;
		Ok(Report { status, matched })
	})
}

struct Run<'a> {
	session: &'a SessionRecord,
	attachment: &'a Attachment,
	file: &'a File,
	deadline: Instant,
	ejection: Eject,
}

impl Run<'_> {
	fn expired(&self) -> bool {
		Instant::now() >= self.deadline
	}

	fn check_source(&self) -> io::Result<()> {
		system::check_attachment_source(self.attachment, self.file).map_err(io::Error::other)
	}

	fn current(&self, store: &SessionStore) -> Result<SessionRecord, Error> {
		let current = store.get(self.session.id())?;
		if current.revision() != self.session.revision()
			|| !matches!(current.phase(), Phase::Mounted(_))
		{
			return Err(Error::SessionChanged);
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
	request: &Request<'_>,
	run: &Run<'_>,
) -> Result<Vec<Target<B::Id>>, Error>
where
	B::Id: Hash + Eq,
	B::Error: Send + Sync,
{
	let mut seen = HashSet::new();
	let mut targets = Vec::with_capacity(request.applications.len());
	for (application, target) in request.applications.iter().zip(request.destinations) {
		let target = absolute(target)?;
		if target.starts_with(run.attachment.mountpoint()) || !seen.insert(target.clone()) {
			return Err(Error::TargetSet);
		}
		let source = application.parse::<VirtualPath>().map_err(Error::copy)?;
		let application = prepare_application(
			view,
			&source,
			NameProfile::PortableAscii,
			NameLimits::default(),
			|| run.expired(),
		)
		.map_err(Error::copy)?;
		let name = std::ffi::OsStr::new(application.destination_name().as_str());
		if target.file_name() != Some(name) {
			return Err(Error::DestinationName);
		}
		let parent = target.parent().ok_or(Error::DestinationName)?;
		let directory = StableDestination::open(parent).map_err(Error::copy)?;
		let plan = prepare_copy(view, application, CopyLimits::default(), || run.expired())
			.map_err(Error::copy)?;
		plan.require_host_metadata().map_err(Error::copy)?;
		targets.push(Target { plan, directory });
	}
	Ok(targets)
}

fn observations<Id>(targets: &[Target<Id>]) -> Result<Vec<Observation>, Error> {
	targets
		.iter()
		.map(|target| {
			let directory = target.directory.directory();
			match observe_existing_bundle(&directory, target.plan.application().destination_name())
			{
				Ok(Some(stamp)) => Ok(Observation::Present(snapshot(stamp))),
				Ok(None) => Ok(Observation::Absent),
				Err(error) if retryable(&error) => Ok(Observation::Unavailable),
				Err(error) => Err(Error::copy(error)),
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

fn retryable<E: StdError + 'static>(error: &ExistingBundleError<E>) -> bool {
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
) -> Result<(Status, usize), Error>
where
	B::Error: Send + Sync,
{
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
					Err(error) => return Err(Error::copy(error)),
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
			_ => return Err(Error::InvalidAction),
		}
	}
}

fn finish<Id>(
	run: &Run<'_>,
	monitor: &mut Monitor<BundleIdentity, BundleStamp>,
	targets: &[Target<Id>],
	proofs: &[Option<VerifiedBundle<'_, Id>>],
	verify_source: &dyn Fn() -> io::Result<()>,
) -> Result<Option<(Status, usize)>, Error> {
	let store = workflow::open_store()?;
	let mut current = run.current(&store)?;
	let matched: Vec<_> = monitor.matched_targets().collect();
	let mut observed = observations(targets)?;
	for &index in &matched {
		if let Err(error) = recheck_target(run, &targets[index], proofs[index].as_ref()) {
			if !retryable(&error) {
				return Err(Error::copy(error));
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
		_ => return Err(Error::InvalidFinalAction),
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
	if run.ejection == Eject::KeepMounted {
		if system::observe(run.attachment)? != system::Observation::Mounted {
			return Err(Error::SessionChanged);
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

/// Outcome of an admitted watch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Status {
	/// Required copies were verified and the mount was left attached.
	Verified,
	/// The recorded mount was detached after verification.
	Detached,
	/// Detachment independently confirmed that the mount was already gone.
	AlreadyDetached,
	/// No matching copy set was established before the deadline.
	TimedOut,
	/// The watch was cancelled.
	Cancelled,
	/// The host refused detachment because the volume was busy.
	Busy,
	/// The watch or detach could not finish.
	Failed,
}

impl Status {
	/// Stable wire name for the completed status.
	#[must_use]
	pub const fn as_str(self) -> &'static str {
		match self {
			Self::Verified => "verified",
			Self::Detached => "detached",
			Self::AlreadyDetached => "already_detached",
			Self::TimedOut => "timed_out",
			Self::Cancelled => "cancelled",
			Self::Busy => "busy",
			Self::Failed => "failed",
		}
	}

	/// Whether the requested operation completed.
	#[must_use]
	pub const fn succeeded(self) -> bool {
		matches!(
			self,
			Self::Verified | Self::Detached | Self::AlreadyDetached
		)
	}
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
