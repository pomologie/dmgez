use dmgez_builder::manifest::Error as ManifestError;

use super::*;

const SOURCE_NAME: &str = "source.bin";
const SOURCE_BYTES: &[u8] = b"admitted source";

#[test]
fn release_staging_refuses_a_stream_plan_without_creating_an_output() {
	let temp = tempfile::tempdir().unwrap();
	let project = Project::parse(MANIFEST).unwrap();
	let manifest = discover(&project, temp.path(), Limits::default()).unwrap();
	let profile = profile(&project, OutputIntent::Stream);
	let plan = BuildPlan::new(&profile, &manifest, RAW.as_slice(), LOGICAL_BYTES as u64).unwrap();
	let result = plan.stage_release(
		&temp.path().join(DESTINATION),
		&|_, _| panic!("stream plan reached verifier"),
		|_| panic!("stream plan started staging"),
	);
	assert!(matches!(result, Err(ExecuteError::Target)));
	assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
}

#[test]
fn release_cancellation_before_verification_cleans_the_stage() {
	let temp = tempfile::tempdir().unwrap();
	let path = temp.path().join(DESTINATION);
	fs::write(&path, OLD_DATA).unwrap();
	let project = Project::parse(MANIFEST).unwrap();
	let manifest = discover(&project, temp.path(), Limits::default()).unwrap();
	let profile = profile_with_release(&project, OutputIntent::Path, ReleaseIntent::Release);
	let plan = BuildPlan::new(&profile, &manifest, RAW.as_slice(), LOGICAL_BYTES as u64).unwrap();
	let result = plan.stage_release(
		&path,
		&|_, _| panic!("cancelled stage reached verifier"),
		|event| {
			if event == BuildProgress::VerifyingOutput {
				profile.write_options().cancellation.cancel();
			}
		},
	);
	assert!(matches!(
		result,
		Err(ExecuteError::Path(PublishError::Validate(
			ImageError::Writer(dmgez_udif::Error::Cancelled)
		)))
	));
	assert_eq!(fs::read(&path).unwrap(), OLD_DATA);
	assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
}

#[test]
fn release_rechecks_source_after_verification_before_returning_the_stage() {
	let temp = tempfile::tempdir().unwrap();
	let root = temp.path().canonicalize().unwrap();
	let source = root.join(SOURCE_NAME);
	let path = root.join(DESTINATION);
	fs::write(&source, SOURCE_BYTES).unwrap();
	fs::write(&path, OLD_DATA).unwrap();
	let project = Project::parse(&format!(
		"{MANIFEST}\n[[contents]]\nsource = '{SOURCE_NAME}'\n"
	))
	.unwrap();
	let manifest = discover(&project, &root, Limits::default()).unwrap();
	let profile = profile_with_release(&project, OutputIntent::Path, ReleaseIntent::Release);
	let plan = BuildPlan::new(&profile, &manifest, RAW.as_slice(), LOGICAL_BYTES as u64).unwrap();
	let result = plan.stage_release(
		&path,
		&|mut stage, _| {
			let mut bytes = [0; LOGICAL_BYTES];
			stage.read_exact(&mut bytes)?;
			assert_eq!(bytes, RAW);
			fs::write(&source, [])
		},
		|_| {},
	);
	assert!(matches!(
		result,
		Err(ExecuteError::Path(PublishError::Validate(
			ImageError::Source(ManifestError::SourceChanged { .. })
		)))
	));
	assert_eq!(fs::read(&path).unwrap(), OLD_DATA);
	assert_eq!(fs::read_dir(&root).unwrap().count(), 2);
}
