//! Synthetic state-machine evidence for D-MOUNT-03-T01 through T03.

use super::*;

type Evidence = Observation<Snapshot<u8, u8>>;

const QUIET: Duration = Duration::from_secs(2);
const TIMEOUT: Duration = Duration::from_secs(20);

fn options(selection: Selection) -> Options {
	Options {
		quiet: QUIET,
		timeout: TIMEOUT,
		selection,
	}
}

const fn found(identity: u8, stamp: u8) -> Evidence {
	Observation::Present(Snapshot { identity, stamp })
}

fn sample(evidence: &Evidence) -> &Snapshot<u8, u8> {
	let Observation::Present(snapshot) = evidence else {
		panic!("expected a present target")
	};
	snapshot
}

fn attempt(action: Action, index: usize) -> Attempt {
	let Action::Verify(attempt) = action else {
		panic!("expected verification, got {action:?}")
	};
	assert_eq!(attempt.target(), index);
	attempt
}

#[test]
fn preexisting_bundle_cannot_become_fresh_from_a_stamp_change_or_reappearance() {
	let start = Instant::now();
	let mut monitor = Monitor::new(start, options(Selection::All), &[found(1, 0)]).unwrap();
	for (delay, observation) in [found(1, 0), found(1, 1), Observation::Absent, found(1, 2)]
		.into_iter()
		.enumerate()
	{
		let time = start + QUIET * u32::try_from(delay).unwrap();
		assert!(matches!(
			monitor.observe(time, &[observation]),
			Ok(Action::Wait)
		));
	}
	assert!(matches!(
		monitor.observe(start + TIMEOUT, &[found(1, 2)]),
		Ok(Action::Attached(StopReason::TimedOut))
	));
}

#[test]
fn replacements_require_a_quiet_snapshot_two_exact_checks_and_at_most_one_ejection() {
	let start = Instant::now();
	let mut monitor = Monitor::new(start, options(Selection::All), &[found(1, 0)]).unwrap();
	let initial_copy = found(2, 1);
	let completed_copy = found(2, 2);
	let changed = start + QUIET;
	let stable = changed + QUIET;
	assert!(matches!(
		monitor.observe(start, &[initial_copy]),
		Ok(Action::Wait)
	));
	assert!(matches!(
		monitor.observe(changed, &[completed_copy]),
		Ok(Action::Wait)
	));
	let requested = attempt(monitor.observe(stable, &[completed_copy]).unwrap(), 0);
	monitor
		.verified(stable, requested, Some(sample(&completed_copy)))
		.unwrap();
	assert!(matches!(
		monitor.observe(stable, &[completed_copy]),
		Ok(Action::Recheck)
	));
	assert!(matches!(
		monitor.recheck(stable, &[completed_copy]),
		Ok(Action::Eject)
	));
	assert!(matches!(
		monitor.observe(stable, &[completed_copy]),
		Ok(Action::Wait)
	));
	assert!(matches!(
		monitor.ejection_finished(start + TIMEOUT, Ejection::Busy),
		Ok(Action::Attached(StopReason::Busy))
	));
	assert!(matches!(
		monitor.observe(start + TIMEOUT, &[completed_copy]),
		Ok(Action::Attached(StopReason::Busy))
	));
}

#[test]
fn a_partial_copy_receipt_and_lost_events_do_not_approve_detachment() {
	let start = Instant::now();
	let mut monitor = Monitor::new(start, options(Selection::All), &[Observation::Absent]).unwrap();
	let copy = found(2, 1);
	let stable = start + QUIET;
	assert!(matches!(monitor.observe(start, &[copy]), Ok(Action::Wait)));
	let requested = attempt(monitor.observe(stable, &[copy]).unwrap(), 0);
	monitor.verified(stable, requested, None).unwrap();
	assert!(matches!(monitor.observe(stable, &[copy]), Ok(Action::Wait)));
	let retried = stable + QUIET;
	let requested = attempt(monitor.observe(retried, &[copy]).unwrap(), 0);
	assert!(matches!(
		monitor.observe(retried, &[Observation::Unavailable]),
		Ok(Action::Wait)
	));
	assert_eq!(
		monitor.verified(retried, requested, Some(sample(&copy))),
		Err(Error::StaleAttempt)
	);
	assert!(matches!(
		monitor.observe(retried, &[copy]),
		Ok(Action::Wait)
	));
}

#[test]
fn a_receipt_from_another_watch_intent_cannot_satisfy_the_pending_check() {
	let start = Instant::now();
	let baseline = [Observation::Absent];
	let mut first = Monitor::new(start, options(Selection::All), &baseline).unwrap();
	let mut other = Monitor::new(start, options(Selection::All), &baseline).unwrap();
	let copy = found(2, 1);
	let verified_at = start + QUIET;
	first.observe(start, &[copy]).unwrap();
	other.observe(start, &[copy]).unwrap();
	let first_attempt = attempt(first.observe(verified_at, &[copy]).unwrap(), 0);
	let other_attempt = attempt(other.observe(verified_at, &[copy]).unwrap(), 0);
	assert_eq!(
		first.verified(verified_at, other_attempt, Some(sample(&copy))),
		Err(Error::StaleAttempt)
	);
	first
		.verified(verified_at, first_attempt, Some(sample(&copy)))
		.unwrap();
	assert!(matches!(
		first.observe(verified_at, &[copy]),
		Ok(Action::Recheck)
	));
}

#[test]
fn selection_all_needs_every_match_and_rechecks_the_current_batch() {
	let start = Instant::now();
	let mut monitor = Monitor::new(
		start,
		options(Selection::All),
		&[Observation::Absent, Observation::Absent],
	)
	.unwrap();
	let left = found(1, 0);
	let right = found(2, 0);
	let stable = start + QUIET;
	let batch = &[left, right];
	assert!(matches!(monitor.observe(start, batch), Ok(Action::Wait)));
	let left_attempt = attempt(monitor.observe(stable, batch).unwrap(), 0);
	monitor
		.verified(stable, left_attempt, Some(sample(&left)))
		.unwrap();
	let right_attempt = attempt(monitor.observe(stable, batch).unwrap(), 1);
	monitor
		.verified(stable, right_attempt, Some(sample(&right)))
		.unwrap();
	assert!(matches!(
		monitor.observe(stable, batch),
		Ok(Action::Recheck)
	));
	assert_eq!(monitor.matched_targets().collect::<Vec<_>>(), vec![0, 1]);
	assert!(matches!(
		monitor.recheck(stable, &[left, found(2, 1)]),
		Ok(Action::Wait)
	));
	assert_eq!(monitor.matched_targets().collect::<Vec<_>>(), vec![0]);
}

#[test]
fn selection_any_can_eject_after_one_fresh_copy_with_two_checks() {
	let start = Instant::now();
	let mut monitor = Monitor::new(
		start,
		options(Selection::Any),
		&[Observation::Absent, Observation::Absent],
	)
	.unwrap();
	let copy = found(1, 0);
	let stable = start + QUIET;
	let batch = &[copy, Observation::Absent];
	monitor.observe(start, batch).unwrap();
	let request = attempt(monitor.observe(stable, batch).unwrap(), 0);
	monitor
		.verified(stable, request, Some(sample(&copy)))
		.unwrap();
	assert!(matches!(
		monitor.observe(stable, batch),
		Ok(Action::Recheck)
	));
	assert!(matches!(monitor.recheck(stable, batch), Ok(Action::Eject)));
	assert!(matches!(
		monitor.ejection_finished(stable, Ejection::Detached),
		Ok(Action::Detached)
	));
}

#[test]
fn a_partial_first_copy_does_not_starve_another_quiet_target() {
	let start = Instant::now();
	let mut monitor = Monitor::new(
		start,
		options(Selection::Any),
		&[Observation::Absent, Observation::Absent],
	)
	.unwrap();
	let first = found(1, 0);
	let second = found(2, 0);
	let batch = &[first, second];
	monitor.observe(start, batch).unwrap();
	let first_attempt = attempt(monitor.observe(start + QUIET, batch).unwrap(), 0);
	monitor
		.verified(start + QUIET, first_attempt, None)
		.unwrap();
	let delayed_poll = start + QUIET * 2;
	let second_attempt = attempt(monitor.observe(delayed_poll, batch).unwrap(), 1);
	monitor
		.verified(delayed_poll, second_attempt, Some(sample(&second)))
		.unwrap();
	assert!(matches!(
		monitor.observe(delayed_poll, batch),
		Ok(Action::Recheck)
	));
	assert!(matches!(
		monitor.recheck(delayed_poll, &[Observation::Unavailable, second]),
		Ok(Action::Eject)
	));
	assert!(matches!(monitor.cancel(), Action::Wait));
	assert!(matches!(
		monitor.ejection_finished(delayed_poll, Ejection::Cancelled),
		Ok(Action::Attached(StopReason::Cancelled))
	));
}

#[test]
fn cancellation_and_deadline_block_pending_rechecks() {
	let start = Instant::now();
	let copy = found(1, 0);
	let stable = start + QUIET;
	for reason in [StopReason::Cancelled, StopReason::TimedOut] {
		let mut monitor =
			Monitor::new(start, options(Selection::All), &[Observation::Absent]).unwrap();
		monitor.observe(start, &[copy]).unwrap();
		let request = attempt(monitor.observe(stable, &[copy]).unwrap(), 0);
		monitor
			.verified(stable, request, Some(sample(&copy)))
			.unwrap();
		assert!(matches!(
			monitor.observe(stable, &[copy]),
			Ok(Action::Recheck)
		));
		if reason == StopReason::Cancelled {
			assert!(matches!(
				monitor.cancel(),
				Action::Attached(StopReason::Cancelled)
			));
		}
		assert!(
			matches!(monitor.recheck(start + TIMEOUT, &[copy]), Ok(Action::Attached(actual)) if actual == reason)
		);
	}
}

#[test]
fn invalid_baselines_timing_and_out_of_order_updates_are_refused() {
	let start = Instant::now();
	let normal = options(Selection::All);
	assert!(matches!(
		Monitor::<u8, u8>::new(start, normal, &[]),
		Err(Error::TargetCount)
	));
	assert!(matches!(
		Monitor::<u8, u8>::new(start, normal, &[Observation::Absent; MAX_TARGETS + 1]),
		Err(Error::TargetCount)
	));
	assert!(matches!(
		Monitor::<u8, u8>::new(start, normal, &[Observation::Unavailable]),
		Err(Error::AmbiguousBaseline)
	));
	assert!(matches!(
		Monitor::<u8, u8>::new(
			start,
			Options {
				timeout: QUIET,
				..normal
			},
			&[Observation::Absent]
		),
		Err(Error::InvalidTiming)
	));
	let mut monitor = Monitor::new(start, normal, &[found(1, 0)]).unwrap();
	assert!(matches!(
		monitor.recheck(start, &[found(2, 0)]),
		Err(Error::UnexpectedTransition)
	));
	assert!(matches!(
		monitor.observe(start + QUIET, &[]),
		Err(Error::TargetCount)
	));
	assert!(matches!(
		monitor.observe(start + QUIET, &[found(2, 0)]),
		Ok(Action::Wait)
	));
	assert!(matches!(
		monitor.observe(start, &[found(2, 0)]),
		Err(Error::ClockReversed)
	));
}
