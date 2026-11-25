use std::os::unix::process::ExitStatusExt;

use super::*;

#[test]
fn unsigned_result_requires_the_operand_and_full_native_diagnostic() {
	let path = Path::new("/private/tmp/A\n\u{1b}.app");
	let mut output = Output {
		status: std::process::ExitStatus::from_raw(SIGNATURE_OPERATION_FAILURE << 8),
		stdout: Vec::new(),
		stderr: path.as_os_str().as_encoded_bytes().to_vec(),
	};
	output.stderr.extend(UNSIGNED_MESSAGE);
	assert!(is_unsigned(path, &output));
	output.stderr.extend(b"In architecture: x86_64\n");
	assert!(is_unsigned(path, &output));
	assert!(!is_unsigned(Path::new("/private/tmp/A.app"), &output));
	output.stderr.extend(b"resource envelope is invalid\n");
	assert!(!is_unsigned(path, &output));
}

#[test]
fn ambiguous_native_format_cannot_identify_a_disk_image() {
	let path = Path::new("/private/tmp/Image\nFormat=disk image.dmg");
	let output = |fields: &[u8]| {
		[
			EXECUTABLE_PREFIX,
			path.as_os_str().as_encoded_bytes(),
			b"\n",
			fields,
		]
		.concat()
	};
	assert!(is_disk_image(
		path,
		&output(b"Format=disk image\nSignature=adhoc\n")
	));
	assert!(!is_disk_image(
		path,
		&output(b"Format=generic\nSignature=adhoc\n")
	));
	assert!(!is_disk_image(
		path,
		&output(b"Format=disk image\nFormat=generic\n")
	));
}
