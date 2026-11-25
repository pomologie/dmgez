#[cfg(not(all(feature = "native", target_os = "macos")))]
#[test]
fn unavailable_native_assessment_does_not_access_the_artifact() {
	use std::path::Path;

	use dmgez_signing::{
		NativeAssessmentStatus, NativeAssessmentUnavailable, assess_native_policy,
	};

	let impossible_path = Path::new("\0");
	let report = assess_native_policy(impossible_path).unwrap();
	assert_eq!(report.artifact, "\0");
	assert_eq!(report.kind, None);
	assert_eq!(report.tool, None);
	assert_eq!(
		report.outcome,
		NativeAssessmentStatus::Unavailable(NativeAssessmentUnavailable::BackendUnavailable)
	);
}
