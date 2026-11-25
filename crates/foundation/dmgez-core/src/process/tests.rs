use std::{
	fs::File,
	path::Path,
	process::{Command, Stdio},
	time::{Duration, Instant},
};

use super::{Error, run};

const TEST_TIMEOUT: Duration = Duration::from_secs(5);
const TEST_OUTPUT_LIMIT: usize = 8;
const SHELL: &str = "/bin/sh";

#[test]
fn sanitizes_the_environment_and_stdin_without_reinterpreting_arguments() {
	const TEST_VARIABLE: &str = "DMGEZ_PROCESS_TEST_VARIABLE";
	const ARGUMENT: &str = "a b; *";
	const PROBE: &str = r#"
		test "${DMGEZ_PROCESS_TEST_VARIABLE-unset}" = unset || exit 1
		test "$LC_ALL" = C || exit 2
		test "$PATH" = /usr/bin:/bin || exit 3
		test "$(pwd)" = / || exit 4
		if IFS= read -r line; then exit 5; fi
		printf '%s' "$1"
	"#;
	let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
	let input = File::open(crate_dir.join("Cargo.toml")).unwrap();
	let mut command = Command::new(SHELL);
	command
		.args(["-c", PROBE, SHELL, ARGUMENT])
		.env(TEST_VARIABLE, "inherited")
		.current_dir(crate_dir)
		.stdin(Stdio::from(input));
	let output = run(&mut command, TEST_TIMEOUT, TEST_OUTPUT_LIMIT).unwrap();
	assert!(output.status.success(), "{output:?}");
	assert_eq!(output.stdout, ARGUMENT.as_bytes());
	assert!(output.stderr.is_empty());
}

#[test]
fn collects_both_streams_at_the_exact_combined_limit() {
	let output = run(
		Command::new(SHELL).args(["-c", "printf data; printf 'err\\377' >&2; exit 7"]),
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
		Command::new(SHELL).args(["-c", "printf left; printf right >&2"]),
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
		Command::new(SHELL).args(["-c", "exec 1>&- 2>&-; exec /bin/sleep 10"]),
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
