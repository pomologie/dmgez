use std::path::Path;

use super::{Relation, attached_device, parse_inventory};
use crate::system::record::{Device, Identity, Timestamp};

const IMAGE: &str = "/Users/example/Example.dmg";
const MOUNTPOINT: &str = "/Users/example/Example";
const DEVICE: &str = "/dev/disk23";

const ID: Identity = Identity {
	device: 91,
	inode: 54,
	created: Timestamp {
		seconds: 7,
		nanos: 8,
	},
};

fn plist(body: &str) -> String {
	format!(
		r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>{body}</dict></plist>"#
	)
}

fn entity(device: &str, mountpoint: &str) -> String {
	format!(
		r"<dict><key>dev-entry</key><string>{device}</string>
<key>mount-point</key><string>{mountpoint}</string>
<key>content-hint</key><string>Apple_HFS</string></dict>"
	)
}

fn entities(entries: &str) -> String {
	format!("<key>system-entities</key><array>{entries}</array>")
}

fn image(path: &str, entries: &str) -> String {
	let devices = entities(entries);
	format!(
		"<dict><key>image-path</key><string>{path}</string>
<key>image-alias</key><data>AA==</data>{devices}</dict>"
	)
}

fn relation(records: &str) -> Relation {
	let inventory = plist(&format!("<key>images</key><array>{records}</array>"));
	let inventory = parse_inventory(inventory.as_bytes()).unwrap();
	inventory.relation(
		Path::new(IMAGE),
		ID,
		Path::new(MOUNTPOINT),
		&Device::try_from(DEVICE.to_owned()).unwrap(),
	)
}

#[test]
fn attach_accepts_only_one_whole_disk_at_the_requested_mountpoint() {
	let expected = Path::new(MOUNTPOINT);
	let valid = plist(&entities(&entity(DEVICE, MOUNTPOINT)));
	assert_eq!(
		attached_device(valid.as_bytes(), expected)
			.unwrap()
			.as_str(),
		DEVICE
	);

	let multiple = entity(DEVICE, MOUNTPOINT) + &entity("/dev/disk24", "/Volumes/Other");
	for invalid in [
		entity("/dev/disk23s1", MOUNTPOINT),
		entity(DEVICE, "/Volumes/Other"),
		entity(DEVICE, "/Users/example//Example"),
		multiple,
	] {
		let xml = plist(&entities(&invalid));
		assert!(attached_device(xml.as_bytes(), expected).is_err());
	}
}

#[test]
fn inventory_requires_a_single_related_record_with_exact_image_device_and_mountpoint() {
	let entry = entity(DEVICE, MOUNTPOINT);
	let expected = image(IMAGE, &entry);
	let unrelated = image(
		"/Volumes/Other.dmg",
		&entity("/dev/disk234", "/Volumes/Other"),
	);
	assert_eq!(relation(""), Relation::Absent);
	assert_eq!(relation(&unrelated), Relation::Absent);
	assert_eq!(relation(&(unrelated + &expected)), Relation::Exact);
	assert_eq!(
		relation(&image("/Volumes/Other.dmg", &entry)),
		Relation::Conflicting
	);
	assert_eq!(
		relation(&image(IMAGE, &entity(DEVICE, "/Volumes/Other"))),
		Relation::Conflicting
	);
	assert_eq!(
		relation(&image(IMAGE, &entity("/dev/disk24", MOUNTPOINT))),
		Relation::Conflicting
	);
	assert_eq!(
		relation(&(expected.clone() + &expected)),
		Relation::Conflicting
	);
	assert_eq!(
		relation(&image(
			"/Volumes/Other.dmg",
			&entity("/dev/disk23s1", "/Volumes/Other")
		)),
		Relation::Conflicting
	);
}

#[test]
fn inventory_rejects_duplicate_or_missing_structural_fields() {
	let fixtures = [
		plist(""),
		plist("<key>images</key><array/><key>images</key><array/>"),
		plist(
			"<key>images</key><array><dict><key>image-path</key><string>/a</string></dict></array>",
		),
		plist(&format!(
			"<key>images</key><array><dict><key>image-path</key><string>{IMAGE}</string>{}{}</dict></array>",
			entities(&entity(DEVICE, MOUNTPOINT)),
			entities(&entity(DEVICE, MOUNTPOINT)),
		)),
	];
	for xml in fixtures {
		assert!(parse_inventory(xml.as_bytes()).is_err());
	}
}
