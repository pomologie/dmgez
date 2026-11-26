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

#[path = "execution/raw.rs"]
mod raw;
#[path = "execution/release_faults.rs"]
mod release_faults;
#[path = "execution/udif_length.rs"]
mod udif_length;

const LOGICAL_BYTES: usize = 512;
const RAW: [u8; LOGICAL_BYTES] = [0xA5; LOGICAL_BYTES];
const DESTINATION: &str = "built.dmg";
const OLD_DATA: &[u8] = b"old destination";
const MANIFEST: &str = "schema_version = 1\n[image]\nname = 'Test'\noutput = 'built.dmg'\nformat = 'udro'\nsize = 512\n";

fn profile(project: &Project, output: OutputIntent) -> ExecutionProfile {
	profile_with_release(project, output, ReleaseIntent::Unsigned)
}

fn profile_with_release(
	project: &Project,
	output: OutputIntent,
	release: ReleaseIntent,
) -> ExecutionProfile {
	let mut capabilities = BuildCapabilities::default();
	capabilities.hfsplus_ascii = Availability::Available;
	capabilities.verify = Availability::Available;
	capabilities.atomic_replace = Availability::Available;
	capabilities.release = Availability::Available;
	preflight(
		project,
		PreflightOptions {
			capabilities,
			output,
			release,
			host_threads: NonZeroUsize::MIN,
		},
	)
	.unwrap()
}

#[test]
fn verifies_a_release_stage_without_publishing_the_destination() {
	let temp = tempfile::tempdir().unwrap();
	let path = temp.path().join(DESTINATION);
	let project = Project::parse(MANIFEST).unwrap();
	let manifest = discover(&project, temp.path(), Limits::default()).unwrap();
	let profile = profile_with_release(&project, OutputIntent::Path, ReleaseIntent::Release);
	let plan = || BuildPlan::new(&profile, &manifest, RAW.as_slice(), RAW.len() as u64).unwrap();
	assert!(matches!(
		plan().publish(&path, None),
		Err(ExecuteError::Release)
	));
	let failed = plan().stage_release(
		&path,
		&|_, _| Err(io::ErrorKind::InvalidData.into()),
		|_| {},
	);
	assert!(matches!(
		failed,
		Err(ExecuteError::Path(PublishError::Validate(
			ImageError::Verification(_)
		)))
	));
	assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
	let events = RefCell::new(Vec::new());
	let mut stage = plan()
		.stage_release(
			&path,
			&|mut reader, report| {
				let mut bytes = Vec::new();
				reader.read_to_end(&mut bytes)?;
				assert!(bytes.starts_with(&RAW));
				assert_eq!(bytes.len() as u64, report.physical_bytes);
				Ok(())
			},
			|event| events.borrow_mut().push(event),
		)
		.unwrap();
	let stage_path = stage.path().to_path_buf();
	assert_eq!(stage_path.parent(), Some(temp.path()));
	assert_eq!(
		stage_path.extension().and_then(std::ffi::OsStr::to_str),
		Some("dmg")
	);
	assert!(!path.exists());
	let mut bytes = [0; LOGICAL_BYTES];
	stage.reader().unwrap().read_exact(&mut bytes).unwrap();
	assert_eq!(bytes, RAW);
	stage.validate_sources().unwrap();
	profile.write_options().cancellation.cancel();
	assert!(matches!(
		stage.validate_sources(),
		Err(ImageError::Writer(dmgez_udif::Error::Cancelled))
	));
	assert_eq!(
		events.into_inner(),
		[
			BuildProgress::WritingImage {
				logical_bytes: RAW.len() as u64
			},
			BuildProgress::VerifyingOutput
		]
	);
	drop(stage);
	assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
}

#[test]
fn rejects_an_ordinary_build_at_the_release_staging_boundary() {
	let temp = tempfile::tempdir().unwrap();
	let project = Project::parse(MANIFEST).unwrap();
	let manifest = discover(&project, temp.path(), Limits::default()).unwrap();
	let ordinary = profile(&project, OutputIntent::Path);
	let plan = BuildPlan::new(&ordinary, &manifest, RAW.as_slice(), RAW.len() as u64).unwrap();
	assert!(matches!(
		plan.stage_release(
			&temp.path().join(DESTINATION),
			&|_, _| panic!("unexpected verifier"),
			|_| {}
		),
		Err(ExecuteError::Release)
	));
	assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
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

fn overwrite_project(contents: &str, verify: bool) -> Project {
	Project::parse(&format!(
		"{MANIFEST}{contents}[build]\noverwrite = true\nverify_output = {verify}\n"
	))
	.unwrap()
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
#[test]
fn overwrite_cannot_fall_back_to_no_clobber_on_an_unsupported_host() {
	let temp = tempfile::tempdir().unwrap();
	let path = temp.path().join(DESTINATION);
	let project = overwrite_project("", false);
	let manifest = discover(&project, temp.path(), Limits::default()).unwrap();
	let profile = profile(&project, OutputIntent::Path);
	let build = BuildPlan::new(&profile, &manifest, RAW.as_slice(), LOGICAL_BYTES as u64).unwrap();
	assert!(matches!(
		build.publish(&path, None),
		Err(ExecuteError::AtomicReplace)
	));
	assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod replacement {
	use super::*;
	use dmgez_builder::manifest::Error as ManifestError;
	use dmgez_core::output::replace::ReplaceError;

	const INPUT_FILE: &str = "input.bin";
	const INPUT_DIR: &str = "input-directory";
	const ALIAS: &str = "alias.dmg";
	const SOURCE_DATA: &[u8] = b"protected source data";
	const MUTATED: &[u8] = b"modified by verifier test";

	#[test]
	fn publishes_a_verified_replacement_only_when_overwrite_is_selected() {
		let temp = tempfile::tempdir().unwrap();
		let directory = temp.path().canonicalize().unwrap();
		let path = directory.join(DESTINATION);
		fs::write(&path, OLD_DATA).unwrap();
		let project = Project::parse(MANIFEST).unwrap();
		let manifest = discover(&project, &directory, Limits::default()).unwrap();
		let no_clobber = profile(&project, OutputIntent::Path);
		let build =
			BuildPlan::new(&no_clobber, &manifest, RAW.as_slice(), LOGICAL_BYTES as u64).unwrap();
		assert!(
			matches!(build.publish(&path, None), Err(ExecuteError::Path(PublishError::Commit(error))) if error.kind() == io::ErrorKind::AlreadyExists)
		);
		assert_eq!(fs::read(&path).unwrap(), OLD_DATA);

		let project = overwrite_project("", true);
		let replace = profile(&project, OutputIntent::Path);
		let build =
			BuildPlan::new(&replace, &manifest, RAW.as_slice(), LOGICAL_BYTES as u64).unwrap();
		let events = RefCell::new(Vec::new());
		let report = build
			.publish_with_progress(
				&path,
				Some(&|mut stage, _| {
					let mut prefix = [0; LOGICAL_BYTES];
					stage.read_exact(&mut prefix)?;
					assert_eq!(prefix, RAW);
					assert_eq!(fs::read(&path)?, OLD_DATA);
					Ok(())
				}),
				|event| events.borrow_mut().push(event),
			)
			.unwrap();
		assert!(report.verified);
		assert!(fs::read(&path).unwrap().starts_with(&RAW));
		assert_eq!(
			events.into_inner(),
			[
				BuildProgress::WritingImage {
					logical_bytes: LOGICAL_BYTES as u64
				},
				BuildProgress::VerifyingOutput,
				BuildProgress::Publishing,
				BuildProgress::Completed {
					physical_bytes: report.image.physical_bytes
				},
			]
		);
		assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
	}

	#[test]
	fn replacement_failures_and_publication_cancellation_keep_the_old_output() {
		let temp = tempfile::tempdir().unwrap();
		let directory = temp.path().canonicalize().unwrap();
		let path = directory.join(DESTINATION);
		fs::write(&path, OLD_DATA).unwrap();
		let project = overwrite_project("", true);
		let manifest = discover(&project, &directory, Limits::default()).unwrap();
		let profile = profile(&project, OutputIntent::Path);
		let build =
			|| BuildPlan::new(&profile, &manifest, RAW.as_slice(), LOGICAL_BYTES as u64).unwrap();
		let failed = build().publish(&path, Some(&|_, _| Err(io::ErrorKind::InvalidData.into())));
		assert!(matches!(
			failed,
			Err(ExecuteError::Replace(ReplaceError::Stage(
				PublishError::Validate(ImageError::Verification(_))
			)))
		));
		assert_eq!(fs::read(&path).unwrap(), OLD_DATA);
		let cancelled =
			build().publish_with_progress(&path, Some(&|_, _| Ok(())), |event| match event {
				BuildProgress::Publishing => profile.write_options().cancellation.cancel(),
				BuildProgress::Completed { .. } => {
					panic!("cancelled replacement reported completion")
				}
				_ => {}
			});
		assert!(matches!(
			cancelled,
			Err(ExecuteError::Replace(ReplaceError::Stage(
				PublishError::Validate(ImageError::Writer(dmgez_udif::Error::Cancelled))
			)))
		));
		assert_eq!(fs::read(&path).unwrap(), OLD_DATA);
		assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
	}

	#[test]
	fn refuses_a_replacement_whose_actual_target_or_parent_aliases_a_source() {
		let temp = tempfile::tempdir().unwrap();
		let directory = temp.path().canonicalize().unwrap();
		let source = directory.join(INPUT_FILE);
		let copied_directory = directory.join(INPUT_DIR);
		let hard_link = directory.join(ALIAS);
		fs::write(&source, SOURCE_DATA).unwrap();
		fs::hard_link(&source, &hard_link).unwrap();
		fs::create_dir(&copied_directory).unwrap();
		let project = overwrite_project(
			&format!(
				"[[contents]]\nsource = '{INPUT_FILE}'\n[[contents]]\nsource = '{INPUT_DIR}'\n"
			),
			false,
		);
		let manifest = discover(&project, &directory, Limits::default()).unwrap();
		let profile = profile(&project, OutputIntent::Path);
		let inside = copied_directory.join(DESTINATION);
		for destination in [&source, &hard_link, &inside] {
			let build =
				BuildPlan::new(&profile, &manifest, RAW.as_slice(), LOGICAL_BYTES as u64).unwrap();
			let result = build
				.publish_with_progress(destination, None, |_| panic!("alias reached the writer"));
			assert!(matches!(
				result,
				Err(ExecuteError::Replace(ReplaceError::Authorize(
					ImageError::Source(ManifestError::OutputSourceAlias { .. })
				)))
			));
		}
		assert_eq!(fs::read(source).unwrap(), SOURCE_DATA);
		assert_eq!(fs::read(hard_link).unwrap(), SOURCE_DATA);
		assert_eq!(fs::read_dir(copied_directory).unwrap().count(), 0);
		assert_eq!(fs::read_dir(directory).unwrap().count(), 3);
	}

	#[test]
	fn replacement_rechecks_source_identity_after_verification() {
		let temp = tempfile::tempdir().unwrap();
		let directory = temp.path().canonicalize().unwrap();
		let source = directory.join(INPUT_FILE);
		let path = directory.join(DESTINATION);
		fs::write(&source, SOURCE_DATA).unwrap();
		fs::write(&path, OLD_DATA).unwrap();
		let project = overwrite_project(&format!("[[contents]]\nsource = '{INPUT_FILE}'\n"), true);
		let manifest = discover(&project, &directory, Limits::default()).unwrap();
		let profile = profile(&project, OutputIntent::Path);
		let build =
			BuildPlan::new(&profile, &manifest, RAW.as_slice(), LOGICAL_BYTES as u64).unwrap();
		let result = build.publish(&path, Some(&|_, _| fs::write(&source, MUTATED)));
		assert!(matches!(
			result,
			Err(ExecuteError::Replace(ReplaceError::Stage(
				PublishError::Validate(ImageError::Source(ManifestError::SourceChanged { .. }))
			)))
		));
		assert_eq!(fs::read(path).unwrap(), OLD_DATA);
		assert_eq!(fs::read(source).unwrap(), MUTATED);
		assert_eq!(fs::read_dir(directory).unwrap().count(), 2);
	}
}
