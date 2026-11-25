use std::{fs, os::unix::process::ExitStatusExt, process::ExitStatus};

use super::*;

const UDZO: &[u8] = include_bytes!(concat!(
	env!("CARGO_MANIFEST_DIR"),
	"/../../../tests/fixtures/images/tiny-UDZO.dmg"
));
const ACCEPTED: &[u8] = b"<plist version='1.0'><dict>\
	<key>assessment:verdict</key><true/>\
	</dict></plist>";
const REJECTED: &[u8] = b"<plist version='1.0'><dict>\
	<key>assessment:verdict</key><false/>\
	</dict></plist>";
const OVERRIDDEN: &[u8] = b"<plist version='1.0'><dict>\
	<key>assessment:verdict</key><true/>\
	<key>assessment:authority</key><dict>\
	<key>assessment:authority:override</key><string>security disabled</string>\
	<key>assessment:authority:verdict</key><false/>\
	</dict></dict></plist>";
const DUPLICATE_VERDICT: &[u8] = b"<plist version='1.0'><dict>\
	<key>assessment:verdict</key><false/>\
	<key>assessment:verdict</key><true/>\
	</dict></plist>";

fn output(code: i32, stdout: &[u8], stderr: &[u8]) -> runner::Output {
	runner::Output {
		status: ExitStatus::from_raw(code << 8),
		stdout: stdout.into(),
		stderr: stderr.into(),
	}
}

fn disk_path(directory: &tempfile::TempDir) -> std::path::PathBuf {
	let image = directory.path().join("--raw\nsource=untrusted.dmg");
	fs::write(&image, UDZO).unwrap();
	image
}

#[test]
fn disk_requires_a_matching_decision_and_rejects_an_override() {
	assert_eq!(
		disk_verdict(&output(0, ACCEPTED, b"")),
		NativeAssessmentStatus::Accepted
	);
	assert_eq!(
		disk_verdict(&output(ASSESSMENT_DENIED, REJECTED, b"")),
		NativeAssessmentStatus::Rejected
	);
	assert_eq!(
		disk_verdict(&output(0, OVERRIDDEN, b"")),
		NativeAssessmentStatus::Unavailable(NativeAssessmentUnavailable::OverriddenVerdict)
	);
	for contradictory in [
		output(0, REJECTED, b""),
		output(ASSESSMENT_DENIED, ACCEPTED, b""),
		output(0, DUPLICATE_VERDICT, b""),
		output(0, ACCEPTED, b"unexpected diagnostic"),
		output(0, b"unrecognized output", b""),
	] {
		assert_eq!(
			disk_verdict(&contradictory),
			NativeAssessmentStatus::Unavailable(NativeAssessmentUnavailable::UnknownToolResponse)
		);
	}
}

#[test]
fn disk_assessment_sends_one_absolute_operand_to_the_open_profile() {
	let directory = tempfile::tempdir().unwrap();
	let input = disk_path(&directory);
	let artifact = Artifact::open(&input).unwrap();
	let mut calls = 0;
	let report = check_with(&artifact, |tool, arguments| {
		assert_eq!(tool, NativeAssessmentTool::Spctl);
		calls += 1;
		if calls == 1 {
			assert_eq!(arguments, STATUS_OPTIONS);
			Ok(output(0, POLICY_ENABLED, b""))
		} else {
			let (path, options) = arguments.split_last().unwrap();
			assert_eq!(options, DISK_OPTIONS);
			assert_eq!(*path, artifact.path().as_os_str());
			assert!(Path::new(path).is_absolute());
			Ok(output(0, ACCEPTED, b""))
		}
	})
	.unwrap();
	assert_eq!(calls, 2);
	assert_eq!(report.kind, Some(ArtifactKind::DiskImage));
	assert_eq!(report.tool, Some(NativeAssessmentTool::Spctl));
	assert_eq!(report.outcome, NativeAssessmentStatus::Accepted);
	assert_eq!(fs::read(input).unwrap(), UDZO);
}

#[test]
fn disabled_or_unavailable_policy_prevents_assessment() {
	let directory = tempfile::tempdir().unwrap();
	let artifact = Artifact::open(&disk_path(&directory)).unwrap();
	let report = check_with(&artifact, |tool, arguments| {
		assert_eq!(tool, NativeAssessmentTool::Spctl);
		assert_eq!(arguments, STATUS_OPTIONS);
		Ok(output(0, POLICY_DISABLED, b""))
	})
	.unwrap();
	assert_eq!(
		report.outcome,
		NativeAssessmentStatus::Unavailable(NativeAssessmentUnavailable::PolicyDisabled)
	);
	#[cfg(feature = "notarization-native")]
	{
		let json = serde_json::to_value(&report).unwrap();
		assert_eq!(json["status"], "unavailable");
		assert_eq!(json["reason"], "policy_disabled");
	}
	let report = check_with(&artifact, |_, arguments| {
		assert_eq!(arguments, STATUS_OPTIONS);
		Err(runner::Error::Spawn(io::ErrorKind::NotFound.into()))
	})
	.unwrap();
	assert_eq!(
		report.outcome,
		NativeAssessmentStatus::Unavailable(NativeAssessmentUnavailable::MissingTool)
	);
}

#[test]
fn app_pass_requires_enabled_policy_before_and_after_assessment() {
	let directory = tempfile::tempdir().unwrap();
	let app = directory.path().join("Example.app");
	fs::create_dir(&app).unwrap();
	let artifact = Artifact::open(&app).unwrap();
	for after in [POLICY_ENABLED, POLICY_DISABLED] {
		let mut calls = 0;
		let report = check_with(&artifact, |tool, arguments| {
			calls += 1;
			if calls == 2 {
				assert_eq!(tool, NativeAssessmentTool::SyspolicyCheck);
				let (path, options) = arguments.split_last().unwrap();
				assert_eq!(options, APP_OPTIONS);
				assert_eq!(*path, artifact.path().as_os_str());
				Ok(output(0, APP_PASSED, b""))
			} else {
				assert_eq!(tool, NativeAssessmentTool::Spctl);
				assert_eq!(arguments, STATUS_OPTIONS);
				Ok(output(
					0,
					if calls == 1 { POLICY_ENABLED } else { after },
					b"",
				))
			}
		})
		.unwrap();
		assert_eq!(calls, 3);
		assert_eq!(report.tool, Some(NativeAssessmentTool::SyspolicyCheck));
		let expected = if after == POLICY_ENABLED {
			NativeAssessmentStatus::Accepted
		} else {
			NativeAssessmentStatus::Unavailable(NativeAssessmentUnavailable::PolicyDisabled)
		};
		assert_eq!(report.outcome, expected);
	}
}

#[test]
fn app_rejection_is_distinct_from_an_unrecognized_response() {
	assert_eq!(
		app_verdict(&output(1, b"", APP_FAILED)),
		NativeAssessmentStatus::Rejected
	);
	for unexpected in [
		output(0, b"", b""),
		output(0, APP_FAILED, b""),
		output(1, APP_PASSED, b""),
	] {
		assert_eq!(
			app_verdict(&unexpected),
			NativeAssessmentStatus::Unavailable(NativeAssessmentUnavailable::UnknownToolResponse)
		);
	}
}

#[test]
fn a_replaced_disk_invalidates_an_affirmative_native_decision() {
	let directory = tempfile::tempdir().unwrap();
	let input = disk_path(&directory);
	let artifact = Artifact::open(&input).unwrap();
	let result = check_with(&artifact, |_, arguments| {
		if arguments == STATUS_OPTIONS {
			return Ok(output(0, POLICY_ENABLED, b""));
		}
		fs::rename(&input, directory.path().join("original.dmg")).unwrap();
		fs::write(&input, UDZO).unwrap();
		Ok(output(0, ACCEPTED, b""))
	});
	assert!(matches!(
		result,
		Err(NativeAssessmentError::Artifact(
			crate::Error::ArtifactChanged { .. }
		))
	));
}
