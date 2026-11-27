use std::cell::Cell;

use dmgez_builder::{manifest::Error as ManifestError, preflight::ReleasePlan};

use super::*;

const SOURCE_NAME: &str = "source.bin";
const SOURCE_BYTES: &[u8] = b"admitted source";
const SIGNING: &str = "\n[signing]\nenabled = true\nmode = 'ad-hoc'\ndmg = true\n";

enum StageAdmission {
	RequiresPath,
	RequiresRelease,
	PrivateStage,
}

#[test]
fn stages_only_explicit_path_releases_without_replacing_the_destination() {
	let temp = tempfile::tempdir().unwrap();
	let path = temp.path().join(DESTINATION);
	fs::write(&path, OLD_DATA).unwrap();
	let project = Project::parse(MANIFEST).unwrap();
	let configured = Project::parse(&format!("{MANIFEST}{SIGNING}")).unwrap();
	let cases = [
		(
			&project,
			OutputIntent::Stream,
			ReleaseIntent::Unsigned,
			ReleasePlan::Unsigned,
			StageAdmission::RequiresPath,
		),
		(
			&project,
			OutputIntent::Path,
			ReleaseIntent::Unsigned,
			ReleasePlan::Unsigned,
			StageAdmission::RequiresRelease,
		),
		(
			&configured,
			OutputIntent::Path,
			ReleaseIntent::Unsigned,
			ReleasePlan::ConfiguredStagesDeferred,
			StageAdmission::RequiresRelease,
		),
		(
			&configured,
			OutputIntent::Path,
			ReleaseIntent::Release,
			ReleasePlan::Requested,
			StageAdmission::PrivateStage,
		),
	];
	for (project, output, intent, release, admission) in cases {
		let manifest = discover(project, temp.path(), Limits::default()).unwrap();
		let profile = profile_with_release(project, output, intent);
		assert_eq!(profile.release(), release);
		let plan =
			BuildPlan::new(&profile, &manifest, RAW.as_slice(), LOGICAL_BYTES as u64).unwrap();
		let verified = Cell::new(false);
		let progressed = Cell::new(false);
		let result = plan.stage_release(
			&path,
			&|mut reader, _| {
				assert!(!verified.replace(true));
				let mut payload = [0; LOGICAL_BYTES];
				reader.read_exact(&mut payload)?;
				assert_eq!(payload, RAW);
				Ok(())
			},
			|_| progressed.set(true),
		);
		match (admission, result) {
			(StageAdmission::RequiresPath, Err(ExecuteError::Target))
			| (StageAdmission::RequiresRelease, Err(ExecuteError::Release)) => {
				assert!(!verified.get() && !progressed.get());
			}
			(StageAdmission::PrivateStage, Ok(stage)) => {
				assert!(verified.get() && progressed.get());
				assert_ne!(stage.path(), path);
				let metadata = fs::metadata(stage.path()).unwrap();
				assert!(metadata.is_file());
				assert_eq!(stage.report().logical_bytes, LOGICAL_BYTES as u64);
				assert_eq!(stage.report().physical_bytes, metadata.len());
				assert_eq!(fs::read(&path).unwrap(), OLD_DATA);
			}
			_ => panic!("unexpected staging result for {output:?} and {release:?}"),
		}
		assert_eq!(fs::read(&path).unwrap(), OLD_DATA);
		assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
	}
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
