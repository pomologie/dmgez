use super::{
	BuildProgress, Command, Error, ExecuteError, IMAGE, OVERWRITE, OutputIntent, PREVIOUS_IMAGE,
	PROJECT_HEADER, Path, Project, REPRODUCIBLE, Stdio, Write, fs, hfsplus, manifest, options,
	planning, presentation, read_published_icon,
};

const BADGE: &str = "badge.png";
const ICNS_BADGE: &str = "badge.icns";
const FIXTURE: &str = concat!(
	env!("CARGO_MANIFEST_DIR"),
	"/tests/smoke/badge_icon_fixture.py"
);

fn project(build_settings: &str, source: &str) -> Project {
	Project::parse(&format!(
		"{PROJECT_HEADER}{build_settings}[volume_icon]\nbadge = '{source}'\n"
	))
	.unwrap()
}

fn write_badge(root: &Path) {
	write_badge_source(root, "--write", BADGE);
}

fn write_badge_source(root: &Path, mode: &str, name: &str) {
	let result = Command::new("python3")
		.args(["-B", FIXTURE, mode])
		.arg(root.join(name))
		.output()
		.unwrap();
	assert!(
		result.status.success(),
		"{}",
		String::from_utf8_lossy(&result.stderr)
	);
}

fn verify_published_badge(output: &Path) {
	let icon = read_published_icon(output);
	let mut oracle = Command::new("python3")
		.args(["-B", FIXTURE, "--verify"])
		.stdin(Stdio::piped())
		.spawn()
		.unwrap();
	oracle.stdin.take().unwrap().write_all(&icon).unwrap();
	assert!(oracle.wait().unwrap().success());
}

#[test]
fn builds_a_deterministic_badge_with_ten_visibly_composited_icon_slots() {
	let config = project(REPRODUCIBLE, BADGE);
	let first = tempfile::tempdir().unwrap();
	let second = tempfile::tempdir().unwrap();
	for root in [first.path(), second.path()] {
		write_badge(root);
		let build = hfsplus::prepare(
			&config,
			root,
			options(OutputIntent::Path),
			Default::default(),
		)
		.unwrap();
		assert!(!root.join(IMAGE).exists());
		assert!(build.prepared().manifest().entries().is_empty());
		assert_eq!(build.prepared().finder_asset_count(), 2);
		build.seal().unwrap().publish(None).unwrap();
	}
	let output = first.path().join(IMAGE);
	assert_eq!(
		fs::read(&output).unwrap(),
		fs::read(second.path().join(IMAGE)).unwrap()
	);
	verify_published_badge(&output);
}

#[test]
fn composites_the_largest_supported_representation_from_an_icns_badge() {
	let temp = tempfile::tempdir().unwrap();
	let root = temp.path();
	write_badge_source(root, "--write-icns", ICNS_BADGE);
	hfsplus::prepare(
		&project(REPRODUCIBLE, ICNS_BADGE),
		root,
		options(OutputIntent::Path),
		Default::default(),
	)
	.unwrap()
	.seal()
	.unwrap()
	.publish(None)
	.unwrap();
	verify_published_badge(&root.join(IMAGE));
}

#[test]
fn rejects_a_malformed_badge_before_replacing_output() {
	let temp = tempfile::tempdir().unwrap();
	let root = temp.path();
	write_badge(root);
	let input = root.join(BADGE);
	let encoded = fs::read(&input).unwrap();
	fs::write(input, &encoded[..encoded.len() / 2]).unwrap();
	let output = root.join(IMAGE);
	fs::write(&output, PREVIOUS_IMAGE).unwrap();
	assert!(matches!(
		hfsplus::prepare(
			&project(OVERWRITE, BADGE),
			root,
			options(OutputIntent::Path),
			Default::default(),
		),
		Err(Error::Prepare(planning::Error::Presentation(
			presentation::Error::VolumeIcon(_)
		)))
	));
	assert_eq!(fs::read(&output).unwrap(), PREVIOUS_IMAGE);
}

#[test]
fn protects_the_badge_source_from_output_alias_and_staged_replacement() {
	let temp = tempfile::tempdir().unwrap();
	let root = temp.path();
	let input = root.join(BADGE);
	let output = root.join(IMAGE);
	let config = project(OVERWRITE, BADGE);
	let prepare = || {
		hfsplus::prepare(
			&config,
			root,
			options(OutputIntent::Path),
			Default::default(),
		)
	};
	write_badge(root);
	fs::hard_link(&input, &output).unwrap();
	assert!(matches!(
		prepare(),
		Err(Error::Prepare(planning::Error::Manifest(
			manifest::Error::OutputSourceAlias { .. }
		)))
	));
	assert_eq!(fs::read(&output).unwrap(), fs::read(&input).unwrap());

	fs::remove_file(&output).unwrap();
	fs::write(&output, PREVIOUS_IMAGE).unwrap();
	let prepared = prepare().unwrap();
	let sealed = prepared.seal().unwrap();
	let result = sealed.publish_with_progress(None, |progress| {
		if matches!(progress, BuildProgress::Publishing) {
			fs::write(&input, b"changed badge during publication").unwrap();
		}
	});
	assert!(matches!(
		result,
		Err(Error::Execute(ExecuteError::Replace(_)))
	));
	assert_eq!(fs::read(&output).unwrap(), PREVIOUS_IMAGE);
	assert_eq!(fs::read_dir(root).unwrap().count(), 2);
}
