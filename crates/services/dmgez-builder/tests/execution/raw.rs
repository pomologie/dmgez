use dmgez_builder::execution::{ImageEncoding, RawWriteError};
use dmgez_config::ImageFormat;

use super::*;

fn raw_project() -> Project {
	let mut project = Project::parse(MANIFEST).unwrap();
	project.image.format = ImageFormat::Udrw;
	project
}

#[test]
fn publishes_exact_raw_bytes_and_preserves_an_existing_output() {
	let root = tempfile::tempdir().unwrap();
	let path = root.path().join(DESTINATION);
	let project = raw_project();
	let manifest = discover(&project, root.path(), Limits::default()).unwrap();
	let profile = profile(&project, OutputIntent::Path);
	let plan = || BuildPlan::new(&profile, &manifest, RAW.as_slice(), RAW.len() as u64).unwrap();
	fs::write(&path, OLD_DATA).unwrap();
	assert!(matches!(
		plan().publish(&path, None),
		Err(ExecuteError::Path(PublishError::Commit(_)))
	));
	assert_eq!(fs::read(&path).unwrap(), OLD_DATA);
	assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
	fs::remove_file(&path).unwrap();
	let report = plan().publish(&path, None).unwrap();
	assert!(matches!(report.image.encoding, ImageEncoding::Udrw));
	assert_eq!(report.image.logical_bytes, RAW.len() as u64);
	assert_eq!(report.image.physical_bytes, report.image.logical_bytes);
	assert!(!report.verified);
	assert_eq!(fs::read(path).unwrap(), RAW);
}

#[test]
fn wrong_raw_lengths_and_cancellation_abandon_the_private_stage() {
	let root = tempfile::tempdir().unwrap();
	let path = root.path().join(DESTINATION);
	let project = raw_project();
	let manifest = discover(&project, root.path(), Limits::default()).unwrap();
	let profile = profile(&project, OutputIntent::Path);
	let plan = |source| BuildPlan::new(&profile, &manifest, source, RAW.len() as u64).unwrap();
	let short = &RAW[..RAW.len() - 1];
	assert!(matches!(
		plan(short).publish(&path, None),
		Err(ExecuteError::Path(PublishError::Write(ImageError::Raw(
			RawWriteError::ShortSource
		))))
	));
	let long = [0; RAW.len() + 1];
	assert!(matches!(
		plan(&long).publish(&path, None),
		Err(ExecuteError::Path(PublishError::Write(ImageError::Raw(
			RawWriteError::LongSource
		))))
	));
	assert!(matches!(
		plan(&RAW).publish_with_progress(&path, None, |event| {
			if matches!(event, BuildProgress::WritingImage { .. }) {
				profile.write_options().cancellation.cancel();
			}
		}),
		Err(ExecuteError::Path(PublishError::Write(ImageError::Raw(
			RawWriteError::Cancelled
		))))
	));
	assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
fn rechecks_the_manifest_after_syncing_the_raw_stage() {
	const SOURCE: &str = "source.bin";
	let temp = tempfile::tempdir().unwrap();
	let root = temp.path().canonicalize().unwrap();
	let path = root.join(DESTINATION);
	let source = root.join(SOURCE);
	fs::write(&source, RAW).unwrap();
	let mut project =
		Project::parse(&format!("{MANIFEST}[[contents]]\nsource = '{SOURCE}'\n")).unwrap();
	project.image.format = ImageFormat::Udrw;
	let manifest = discover(&project, &root, Limits::default()).unwrap();
	let profile = profile(&project, OutputIntent::Path);
	let plan = BuildPlan::new(&profile, &manifest, RAW.as_slice(), RAW.len() as u64).unwrap();
	let result = plan.publish_with_progress(&path, None, |event| {
		if matches!(event, BuildProgress::Publishing) {
			fs::write(&source, OLD_DATA).unwrap();
		}
	});
	assert!(matches!(
		result,
		Err(ExecuteError::Path(PublishError::Validate(
			ImageError::Source(_)
		)))
	));
	assert_eq!(fs::read(&source).unwrap(), OLD_DATA);
	assert!(!path.exists());
	assert_eq!(fs::read_dir(root).unwrap().count(), 1);
}
