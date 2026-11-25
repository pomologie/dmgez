//! State transitions for verified external app copies.
//!
//! Callers bind one monitor to a single mount and a fixed ordered list of
//! authorized destinations. The monitor grants a single normal eject attempt.
//! The host still needs to revalidate the mount before attempting detachment.

use std::{
	fmt,
	sync::Arc,
	time::{Duration, Instant},
};

/// Maximum number of destinations admitted under one watch intent.
pub const MAX_TARGETS: usize = 32;

/// A named destination's persistent identity and its current change stamp.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Snapshot<I, S> {
	/// Identity of the destination directory itself.
	pub identity: I,
	/// Observation of the current named directory revision.
	pub stamp: S,
}

/// A bounded observation relative to the authorized destination parent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Observation<T> {
	Absent,
	Present(T),
	Unavailable,
}

/// Which fresh verified copies are necessary for approval.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Selection {
	#[default]
	All,
	Any,
}

/// Time and completion policy for one watch intent.
#[derive(Clone, Copy, Debug)]
pub struct Options {
	pub quiet: Duration,
	pub timeout: Duration,
	pub selection: Selection,
}

/// An admitted copy verification request. Its receipt can only be applied to
/// the monitor that issued it, against the still-current destination.
#[derive(Debug)]
pub struct Attempt {
	target: usize,
	token: Arc<()>,
}

impl Attempt {
	/// Index in the baseline and every observation batch.
	#[must_use]
	pub const fn target(&self) -> usize {
		self.target
	}
}

/// The next step requested by the monitor.
#[derive(Debug)]
pub enum Action {
	Wait,
	Verify(Attempt),
	/// Revalidate each matched revision against the retained verification evidence.
	Recheck,
	/// Attempt ordinary detachment of the exact managed mount once.
	Eject,
	Attached(StopReason),
	Detached,
}

/// Why monitoring ended with the image still attached.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StopReason {
	TimedOut,
	Cancelled,
	Busy,
	Failed,
}

/// Result of the authorized normal detach attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ejection {
	Detached,
	Busy,
	/// Detachment failed and the host still confirms the attachment.
	Failed,
	/// The caller cancelled before starting the native detach.
	Cancelled,
}

/// An invalid watch request or transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
	TargetCount,
	AmbiguousBaseline,
	InvalidTiming,
	ClockReversed,
	StaleAttempt,
	UnexpectedTransition,
}

impl fmt::Display for Error {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.write_str(match self {
			Self::TargetCount => "the watch target count is invalid or changed",
			Self::AmbiguousBaseline => "the initial destination could not be confirmed",
			Self::InvalidTiming => "the watch timeout must exceed a positive quiet period",
			Self::ClockReversed => "the watch clock moved backwards",
			Self::StaleAttempt => "the destination verification request is no longer current",
			Self::UnexpectedTransition => "this watch action is not pending",
		})
	}
}

impl std::error::Error for Error {}

/// Bounded state for one admitted watch intent.
pub struct Monitor<I, S> {
	targets: Vec<Target<I, S>>,
	quiet: Duration,
	deadline: Instant,
	selection: Selection,
	now: Instant,
	phase: Phase,
}

struct Target<I, S> {
	baseline: Option<I>,
	candidate: Option<Candidate<I, S>>,
}

struct Candidate<I, S> {
	snapshot: Snapshot<I, S>,
	since: Instant,
	verified: bool,
}

enum Phase {
	Watching,
	Verifying(Attempt),
	Rechecking,
	Ejecting,
	Attached(StopReason),
	Detached,
}

impl<I: Clone + Eq, S: Clone + Eq> Monitor<I, S> {
	/// Admit an intent using exact destination baselines captured before watching.
	///
	/// # Errors
	/// Refuses invalid timing, an empty or oversized target list, and any
	/// ambiguous baseline.
	pub fn new(
		start: Instant,
		options: Options,
		baseline: &[Observation<Snapshot<I, S>>],
	) -> Result<Self, Error> {
		if !(1..=MAX_TARGETS).contains(&baseline.len()) {
			return Err(Error::TargetCount);
		}
		if options.quiet.is_zero() || options.timeout <= options.quiet {
			return Err(Error::InvalidTiming);
		}
		let deadline = start
			.checked_add(options.timeout)
			.ok_or(Error::InvalidTiming)?;
		let mut targets = Vec::with_capacity(baseline.len());
		for observation in baseline {
			let baseline = match observation {
				Observation::Absent => None,
				Observation::Present(snapshot) => Some(snapshot.identity.clone()),
				Observation::Unavailable => return Err(Error::AmbiguousBaseline),
			};
			targets.push(Target {
				baseline,
				candidate: None,
			});
		}
		Ok(Self {
			targets,
			quiet: options.quiet,
			deadline,
			selection: options.selection,
			now: start,
			phase: Phase::Watching,
		})
	}

	/// Observe each authorized target in the original order. Provide
	/// `Unavailable` for an ambiguous observation or an affected target when
	/// watcher events were lost. A newly observed candidate must stay stable
	/// across the quiet period before verification begins.
	///
	/// # Errors
	/// Rejects a changed target count, backwards time, or an outstanding final
	/// recheck. Complete the recheck or cancel before submitting another batch.
	pub fn observe(
		&mut self,
		now: Instant,
		observed: &[Observation<Snapshot<I, S>>],
	) -> Result<Action, Error> {
		self.check_count(observed)?;
		self.advance(now)?;
		if let Some(action) = self.complete_action() {
			return Ok(action);
		}
		if matches!(self.phase, Phase::Rechecking) {
			return Err(Error::UnexpectedTransition);
		}
		if matches!(self.phase, Phase::Ejecting) {
			return Ok(Action::Wait);
		}
		let verifying = match &self.phase {
			Phase::Verifying(attempt) => Some(attempt.target),
			_ => None,
		};
		for (index, (target, evidence)) in self.targets.iter_mut().zip(observed).enumerate() {
			if target.update(now, evidence) && verifying == Some(index) {
				self.phase = Phase::Watching;
			}
		}
		if matches!(self.phase, Phase::Verifying(_)) {
			return Ok(Action::Wait);
		}
		if self
			.selection
			.enough(self.matched_targets().count(), self.targets.len())
		{
			self.phase = Phase::Rechecking;
			return Ok(Action::Recheck);
		}
		let oldest_ready = self
			.targets
			.iter()
			.enumerate()
			.filter_map(|(index, target)| {
				target
					.candidate
					.as_ref()
					.map(|candidate| (index, candidate))
			})
			.filter(|(_, candidate)| {
				!candidate.verified && now.duration_since(candidate.since) >= self.quiet
			})
			.min_by_key(|(_, candidate)| candidate.since);
		if let Some((target, _)) = oldest_ready {
			let token = Arc::new(());
			self.phase = Phase::Verifying(Attempt {
				target,
				token: Arc::clone(&token),
			});
			return Ok(Action::Verify(Attempt { target, token }));
		}
		Ok(Action::Wait)
	}

	/// Apply one held-source verification result. A matching receipt contains
	/// the exact destination snapshot before and after content and metadata
	/// verification. Use `None` for a partial, failed or inconclusive check.
	///
	/// # Errors
	/// Rejects a receipt for another monitor or an invalidated candidate, and
	/// backwards time. Receipts arriving after monitoring ends have no effect.
	pub fn verified(
		&mut self,
		now: Instant,
		attempt: Attempt,
		evidence: Option<&Snapshot<I, S>>,
	) -> Result<(), Error> {
		self.advance(now)?;
		if self.complete_action().is_some() {
			return Ok(());
		}
		if !matches!(&self.phase, Phase::Verifying(current) if Arc::ptr_eq(&current.token, &attempt.token))
		{
			return Err(Error::StaleAttempt);
		}
		let candidate = self.targets[attempt.target]
			.candidate
			.as_mut()
			.ok_or(Error::StaleAttempt)?;
		candidate.verified = evidence == Some(&candidate.snapshot);
		if !candidate.verified {
			candidate.since = now;
		}
		self.phase = Phase::Watching;
		Ok(())
	}

	/// Indexes whose current revision passed the full comparison.
	pub fn matched_targets(&self) -> impl Iterator<Item = usize> + '_ {
		self.targets
			.iter()
			.enumerate()
			.filter_map(|(index, target)| {
				target
					.candidate
					.as_ref()
					.filter(|candidate| candidate.verified)
					.map(|_| index)
			})
	}

	/// Complete a requested final recheck. Supply a previously verified snapshot
	/// only after checking that the retained source and full destination tree
	/// evidence are current. Supply `Unavailable` for an incomplete or ambiguous
	/// comparison.
	///
	/// # Errors
	/// Rejects a changed target count, backwards time, or a receipt without a
	/// pending final recheck.
	pub fn recheck(
		&mut self,
		now: Instant,
		observed: &[Observation<Snapshot<I, S>>],
	) -> Result<Action, Error> {
		self.check_count(observed)?;
		self.advance(now)?;
		if let Some(action) = self.complete_action() {
			return Ok(action);
		}
		if !matches!(self.phase, Phase::Rechecking) {
			return Err(Error::UnexpectedTransition);
		}
		let mut confirmed = 0;
		for (target, observation) in self.targets.iter_mut().zip(observed) {
			target.update(now, observation);
			if target
				.candidate
				.as_ref()
				.is_some_and(|candidate| candidate.verified)
			{
				confirmed += 1;
			}
		}
		if self.selection.enough(confirmed, self.targets.len()) {
			self.phase = Phase::Ejecting;
			Ok(Action::Eject)
		} else {
			self.phase = Phase::Watching;
			Ok(Action::Wait)
		}
	}

	/// Stop before issuing detachment. If detachment is already handed off, the
	/// caller has to report its actual outcome, or acknowledge cancellation
	/// before the host call with `Ejection::Cancelled`.
	pub fn cancel(&mut self) -> Action {
		if let Some(action) = self.complete_action() {
			return action;
		}
		if matches!(self.phase, Phase::Ejecting) {
			return Action::Wait;
		}
		self.phase = Phase::Attached(StopReason::Cancelled);
		Action::Attached(StopReason::Cancelled)
	}

	/// Record the sole detach attempt. A busy or failed operation ends this
	/// authorization and leaves further detachment to the user.
	///
	/// # Errors
	/// Rejects a result that does not have a pending eject, and backwards time.
	pub fn ejection_finished(&mut self, now: Instant, result: Ejection) -> Result<Action, Error> {
		self.advance(now)?;
		if !matches!(self.phase, Phase::Ejecting) {
			return Err(Error::UnexpectedTransition);
		}
		let reason = match result {
			Ejection::Detached => {
				self.phase = Phase::Detached;
				return Ok(Action::Detached);
			}
			Ejection::Busy => StopReason::Busy,
			Ejection::Failed => StopReason::Failed,
			Ejection::Cancelled => StopReason::Cancelled,
		};
		self.phase = Phase::Attached(reason);
		Ok(Action::Attached(reason))
	}

	fn check_count(&self, observed: &[Observation<Snapshot<I, S>>]) -> Result<(), Error> {
		if self.targets.len() == observed.len() {
			Ok(())
		} else {
			Err(Error::TargetCount)
		}
	}

	fn advance(&mut self, now: Instant) -> Result<(), Error> {
		if now < self.now {
			return Err(Error::ClockReversed);
		}
		self.now = now;
		if now >= self.deadline
			&& matches!(
				self.phase,
				Phase::Watching | Phase::Verifying(_) | Phase::Rechecking
			) {
			self.phase = Phase::Attached(StopReason::TimedOut);
		}
		Ok(())
	}

	fn complete_action(&self) -> Option<Action> {
		match self.phase {
			Phase::Attached(reason) => Some(Action::Attached(reason)),
			Phase::Detached => Some(Action::Detached),
			_ => None,
		}
	}
}

impl<I: Clone + Eq, S: Clone + Eq> Target<I, S> {
	fn update(&mut self, now: Instant, observed: &Observation<Snapshot<I, S>>) -> bool {
		let Observation::Present(snapshot) = observed else {
			return self.candidate.take().is_some();
		};
		if self.baseline.as_ref() == Some(&snapshot.identity) {
			return self.candidate.take().is_some();
		}
		if self
			.candidate
			.as_ref()
			.is_some_and(|candidate| &candidate.snapshot == snapshot)
		{
			return false;
		}
		self.candidate = Some(Candidate {
			snapshot: snapshot.clone(),
			since: now,
			verified: false,
		});
		true
	}
}

impl Selection {
	fn enough(self, confirmed: usize, selected: usize) -> bool {
		match self {
			Self::All => confirmed == selected,
			Self::Any => confirmed != 0,
		}
	}
}

#[cfg(test)]
mod tests;
