//! Staged and stream output checks for D-BUILD-05 and D-V06.

use std::{
	cell::RefCell,
	fs,
	io::{self, Read},
	num::NonZeroUsize,
	path::Path,
};

use dmgez_builder::{
	execution::{BuildPlan, ExecuteError, ImageError},
	manifest::{Limits, discover},
	preflight::{
		Availability, BuildCapabilities, ExecutionProfile, OutputIntent, PreflightOptions,
		ReleaseIntent, preflight,
	},
	progress::BuildProgress,
};
use dmgez_config::Project;
use dmgez_core::output::PublishError;

const LOGICAL_BYTES: usize = 512;
const RAW: [u8; LOGICAL_BYTES] = [0xA5; LOGICAL_BYTES];
const DESTINATION: &str = "built.dmg";
const OLD_DATA: &[u8] = b"old destination";
const MANIFEST: &str = "schema_version = 1\n[image]\nname = 'Test'\noutput = 'built.dmg'\nformat = 'udro'\nsize = 512\n";

fn profile(project: &Project, output: OutputIntent) -> ExecutionProfile {
	let mut capabilities = BuildCapabilities::default();
	capabilities.hfsplus_ascii = Availability::Available;
	capabilities.verify = Availability::Available;
	preflight(
		project,
		PreflightOptions {
			capabilities,
			output,
			release: ReleaseIntent::Unsigned,
			host_threads: NonZeroUsize::MIN,
		},
	)
	.unwrap()
}

#[test]
fn path_output_requires_requested_verification_and_preserves_an_existing_file() {
	let temp = tempfile::tempdir().unwrap();
	let path = temp.path().join(DESTINATION);
	let (project, manifest) = verified_project(temp.path());
	let profile = profile(&project, OutputIntent::Path);
	let plan =
		|| BuildPlan::new(&profile, &manifest, RAW.as_slice(), LOGICAL_BYTES as u64).unwrap();
	assert!(matches!(
		plan().publish(&path, None),
		Err(ExecuteError::VerifierRequired)
	));
	assert!(!path.exists());

	fs::write(&path, OLD_DATA).unwrap();
	let failed = plan().publish(
		&path,
		Some(&|mut reader, report| {
			let mut stage_prefix = [0; LOGICAL_BYTES];
			reader.read_exact(&mut stage_prefix)?;
			assert_eq!(stage_prefix, RAW);
			assert!(report.physical_bytes > LOGICAL_BYTES as u64);
			Err(io::ErrorKind::InvalidData.into())
		}),
	);
	assert!(matches!(
		failed,
		Err(ExecuteError::Path(PublishError::Validate(
			ImageError::Verification(_)
		)))
	));
	assert_eq!(fs::read(&path).unwrap(), OLD_DATA);
	assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);

	fs::remove_file(&path).unwrap();
	let events = RefCell::new(Vec::new());
	let accepted = plan()
		.publish_with_progress(
			&path,
			Some(&|mut reader, report| {
				let mut stage = Vec::new();
				reader.read_to_end(&mut stage)?;
				assert_eq!(stage.len() as u64, report.physical_bytes);
				assert!(stage.starts_with(&RAW));
				Ok(())
			}),
			|event| {
				assert_eq!(
					path.exists(),
					matches!(event, BuildProgress::Completed { .. })
				);
				events.borrow_mut().push(event);
			},
		)
		.unwrap();
	assert!(accepted.verified);
	assert_eq!(
		fs::metadata(path).unwrap().len(),
		accepted.image.physical_bytes
	);
	assert_eq!(
		events.into_inner(),
		[
			BuildProgress::WritingImage {
				logical_bytes: LOGICAL_BYTES as u64
			},
			BuildProgress::VerifyingOutput,
			BuildProgress::Publishing,
			BuildProgress::Completed {
				physical_bytes: accepted.image.physical_bytes
			},
		]
	);
}

#[test]
fn cancellation_at_publication_reports_no_completion_and_cleans_up() {
	let temp = tempfile::tempdir().unwrap();
	let path = temp.path().join(DESTINATION);
	let project = Project::parse(MANIFEST).unwrap();
	let manifest = discover(&project, temp.path(), Limits::default()).unwrap();
	let profile = profile(&project, OutputIntent::Path);
	let plan = BuildPlan::new(&profile, &manifest, RAW.as_slice(), LOGICAL_BYTES as u64).unwrap();
	let result = plan.publish_with_progress(&path, None, |event| match event {
		BuildProgress::Publishing => profile.write_options().cancellation.cancel(),
		BuildProgress::Completed { .. } => panic!("cancelled build reported completion"),
		_ => {}
	});
	assert!(matches!(
		result,
		Err(ExecuteError::Path(PublishError::Validate(
			ImageError::Writer(dmgez_udif::Error::Cancelled)
		)))
	));
	assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
}

#[test]
fn streams_report_failure_and_cancelled_paths_create_no_stage() {
	let temp = tempfile::tempdir().unwrap();
	let project = Project::parse(MANIFEST).unwrap();
	let manifest = discover(&project, temp.path(), Limits::default()).unwrap();
	let stream = profile(&project, OutputIntent::Stream);
	let plan = BuildPlan::new(&stream, &manifest, RAW.as_slice(), LOGICAL_BYTES as u64).unwrap();
	let mut short_sink = [0; 3];
	assert!(matches!(
		plan.write_to(&mut short_sink[..]),
		Err(ExecuteError::Stream(_))
	));
	assert!(short_sink.iter().all(|&byte| byte == RAW[0]));

	let path_profile = profile(&project, OutputIntent::Path);
	path_profile.write_options().cancellation.cancel();
	let plan = BuildPlan::new(
		&path_profile,
		&manifest,
		RAW.as_slice(),
		LOGICAL_BYTES as u64,
	)
	.unwrap();
	assert!(matches!(
		plan.publish(&temp.path().join(DESTINATION), None),
		Err(ExecuteError::BeforeOutput(ImageError::Writer(
			dmgez_udif::Error::Cancelled
		)))
	));
	assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
}

fn verified_project(directory: &Path) -> (Project, dmgez_builder::manifest::Manifest) {
	let project = Project::parse(&format!("{MANIFEST}[build]\nverify_output = true\n")).unwrap();
	let manifest = discover(&project, directory, Limits::default()).unwrap();
	(project, manifest)
}
