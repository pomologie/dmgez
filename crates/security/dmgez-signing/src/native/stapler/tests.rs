use std::{io, os::unix::process::ExitStatusExt};

use super::*;

#[test]
fn native_arguments_keep_the_absolute_operand_intact() {
	let path = Path::new("/private/tmp/--option with spaces\nand a newline.dmg");
	for (action, error) in [
		(STAPLE, Error::Staple as fn(ExitStatus) -> Error),
		(VALIDATE, Error::Validate),
	] {
		request(path, action, error, |args| {
			assert_eq!(
				args,
				&[OsStr::new(action), OsStr::new(QUIET), path.as_os_str()]
			);
			Ok(output(0))
		})
		.unwrap();
	}
}

#[test]
fn rejects_a_relative_operand_before_launch() {
	let result = request(Path::new("--help"), STAPLE, Error::Staple, |_| {
		panic!("relative paths must not reach the tool")
	});
	assert!(matches!(result, Err(Error::RelativePath)));
}

#[test]
fn reports_failures_without_exposing_native_output() {
	let path = Path::new("/private/tmp/fixture.dmg");
	let staple = request(path, STAPLE, Error::Staple, |_| Ok(output(65))).unwrap_err();
	assert!(matches!(staple, Error::Staple(status) if status.code() == Some(65)));
	assert!(!staple.to_string().contains("private"));
	assert!(!format!("{staple:?}").contains("private"));

	let validate = request(path, VALIDATE, Error::Validate, |_| Ok(output(65))).unwrap_err();
	assert!(matches!(validate, Error::Validate(status) if status.code() == Some(65)));
	assert!(!validate.to_string().contains("private"));

	let process_error = request(path, STAPLE, Error::Staple, |_| {
		Err(runner::Error::Spawn(io::ErrorKind::NotFound.into()))
	});
	assert!(matches!(process_error, Err(Error::Tool(_))));
}

fn output(code: i32) -> runner::Output {
	runner::Output {
		status: ExitStatus::from_raw(code << 8),
		stdout: b"private stdout".to_vec(),
		stderr: b"private stderr".to_vec(),
	}
}
