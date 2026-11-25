use std::{
	process::Command,
	time::{Duration, Instant},
};

use super::{Error, run};

const TEST_TIMEOUT: Duration = Duration::from_secs(5);
const TEST_OUTPUT_LIMIT: usize = 8;

#[test]
fn collects_both_streams_at_the_exact_combined_limit() {
	let output = run(
		Command::new("/bin/sh").args(["-c", "printf data; printf 'err\\377' >&2; exit 7"]),
		TEST_TIMEOUT,
		TEST_OUTPUT_LIMIT,
	)
	.unwrap();
	assert_eq!(output.status.code(), Some(7));
	assert_eq!(output.stdout, b"data");
	assert_eq!(output.stderr, b"err\xff");
}

#[test]
fn refuses_output_beyond_the_combined_limit() {
	let result = run(
		Command::new("/bin/sh").args(["-c", "printf left; printf right >&2"]),
		TEST_TIMEOUT,
		TEST_OUTPUT_LIMIT,
	);
	assert!(matches!(
		result,
		Err(Error::OutputLimit {
			limit: TEST_OUTPUT_LIMIT
		})
	));
}

#[test]
fn times_out_when_a_child_closes_output_pipes_but_keeps_running() {
	const SHORT_TIMEOUT: Duration = Duration::from_millis(100);
	let started = Instant::now();
	let result = run(
		Command::new("/bin/sh").args(["-c", "exec 1>&- 2>&-; exec /bin/sleep 10"]),
		SHORT_TIMEOUT,
		TEST_OUTPUT_LIMIT,
	);
	assert!(matches!(
		result,
		Err(Error::Timeout {
			timeout: SHORT_TIMEOUT
		})
	));
	assert!(started.elapsed() < TEST_TIMEOUT);
}
