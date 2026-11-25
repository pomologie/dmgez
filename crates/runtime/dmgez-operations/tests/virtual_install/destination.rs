use super::*;

#[test]
fn refuses_dangling_output_links_and_replaced_destination_directories() {
	use dmgez_operations::extract::host::{HostError, StableDestination};
	use std::{fs, os::unix::fs::symlink};

	let reads = Cell::new(0);
	let mut view = Fixture::new(&reads).view();
	let plan = prepare_application(
		&mut view,
		&SELECTED_APP.parse().unwrap(),
		QUALIFIED_PROFILE,
		NameLimits::default(),
		|| false,
	)
	.unwrap();
	let parent = tempfile::tempdir().unwrap();
	let path = parent.path().canonicalize().unwrap().join("destination");
	fs::create_dir(&path).unwrap();
	let destination = StableDestination::open(&path).unwrap();
	plan.require_fresh_destination(&destination).unwrap();
	assert_eq!(fs::read_dir(&path).unwrap().count(), 0);
	let output = path.join(APP_NAME);
	symlink("missing", &output).unwrap();
	assert!(matches!(
		plan.require_fresh_destination(&destination),
		Err(HostError::Conflict)
	));
	assert!(output.symlink_metadata().unwrap().is_symlink());
	fs::rename(&path, path.with_extension("previous")).unwrap();
	fs::create_dir(&path).unwrap();
	assert!(matches!(
		plan.require_fresh_destination(&destination),
		Err(HostError::Detached)
	));
}
