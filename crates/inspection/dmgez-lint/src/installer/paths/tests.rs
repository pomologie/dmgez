use dmgez_builder::manifest::FinderOverrides;

use super::*;

fn entry(destination: &str, kind: EntryKind) -> Entry {
	Entry {
		destination: destination.parse().unwrap(),
		kind,
		source: None,
		finder: FinderOverrides::default(),
	}
}

fn findings(entries: &[Entry]) -> Vec<Evidence> {
	let mut findings = Vec::new();
	check_paths(
		entries,
		|evidence| {
			findings.push(evidence);
			Ok(())
		},
		&mut || false,
	)
	.unwrap();
	findings
}

#[test]
fn top_level_app_requires_the_root_shortcut() {
	let app = entry("/Release.APP", EntryKind::Directory);
	assert!(matches!(
		findings(std::slice::from_ref(&app)).as_slice(),
		[Evidence::MissingApplicationsLink { destination }]
			if destination == APPLICATIONS_DESTINATION
	));
	let shortcut = entry(
		APPLICATIONS_DESTINATION,
		EntryKind::Symlink(APPLICATIONS_TARGET.to_vec()),
	);
	assert!(findings(&[app, shortcut]).is_empty());
	assert!(
		findings(&[
			entry("/Nested/Release.app", EntryKind::Directory),
			entry("/Portable.app", EntryKind::File),
		])
		.is_empty()
	);
}

#[test]
fn wrong_shortcut_is_reported_once_with_exact_target() {
	let wrong_target = b"/Different";
	let shortcut = entry(
		APPLICATIONS_DESTINATION,
		EntryKind::Symlink(wrong_target.to_vec()),
	);
	assert!(matches!(
		findings(&[shortcut]).as_slice(),
		[Evidence::InvalidApplicationsLink { destination, target: Some(target) }]
			if destination == APPLICATIONS_DESTINATION && target == wrong_target
	));
	assert!(matches!(
		findings(&[entry(APPLICATIONS_DESTINATION, EntryKind::Directory)]).as_slice(),
		[Evidence::InvalidApplicationsLink { destination, target: None }]
			if destination == APPLICATIONS_DESTINATION
	));
}

#[test]
fn other_links_may_not_leave_the_image_lexically() {
	let root_escape = b"dir//.//../../bad\xff";
	assert!(escapes_root(root_escape, 0));
	assert!(!escapes_root(root_escape, 1));
	assert!(escapes_root(APPLICATIONS_TARGET, 2));
	assert!(!escapes_root(b"./local//../inside\xff", 0));
	let destination = "/Nested/Shortcut";
	let link = entry(
		destination,
		EntryKind::Symlink(APPLICATIONS_TARGET.to_vec()),
	);
	assert!(matches!(
		findings(&[link]).as_slice(),
		[Evidence::ExternalSymlink { destination: path, target }]
			if path == destination && target == APPLICATIONS_TARGET
	));
}
