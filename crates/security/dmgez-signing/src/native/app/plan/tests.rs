use std::{
	fs,
	os::unix::fs::{PermissionsExt, symlink},
	path::Path,
};

use super::{Error, build};

const APP: &str = "Fixture.app";
const ROOT_MAIN: &str = "Main";
const SCRIPT: &[u8] = b"#!/bin/sh\nexit 0\n";
const ROOT_PLIST: &str = "Contents/Info.plist";
const ROOT_BIN: &str = "Contents/MacOS";
const HELPER: &str = "Contents/Helpers/Helper";
const PLUGIN: &str = "Contents/PlugIns/Action.appex";
const CODELESS: &str = "Contents/PlugIns/Resource.bundle";
const SYSTEM_EXTENSION: &str = "Contents/Library/SystemExtensions/Network.systemextension";
const DRIVER_EXTENSION: &str = "Contents/Library/SystemExtensions/Device.dext";
const FRAMEWORK_ROOT: &str = "Contents/Frameworks/Widget.framework";
const FRAMEWORK_A: &str = "Contents/Frameworks/Widget.framework/Versions/A";
const FRAMEWORK_B: &str = "Contents/Frameworks/Widget.framework/Versions/B";
const FRAMEWORK_MAIN: &str = "Widget";
const FRAMEWORK_INFO: &str = "Resources/Info.plist";

#[test]
fn orders_bundles_libraries_and_helpers_inside_out() {
	let temp = tempfile::tempdir().unwrap();
	let app = temp.path().join(APP);
	make_bundle(&app, ROOT_MAIN, SCRIPT);
	write_executable(&app.join(HELPER), &macho(false));
	make_bundle(&app.join(PLUGIN), "Action", &macho(false));
	write_info(&app.join(CODELESS).join(ROOT_PLIST), None);
	write_info(
		&app.join(FRAMEWORK_A).join(FRAMEWORK_INFO),
		Some(FRAMEWORK_MAIN),
	);
	write_info(
		&app.join(FRAMEWORK_B).join(FRAMEWORK_INFO),
		Some(FRAMEWORK_MAIN),
	);
	write_executable(&app.join(FRAMEWORK_A).join(FRAMEWORK_MAIN), &macho(true));
	write_executable(&app.join(FRAMEWORK_B).join(FRAMEWORK_MAIN), &macho(true));
	symlink("A", app.join(FRAMEWORK_ROOT).join("Versions/Current")).unwrap();
	symlink(
		"Versions/Current/Widget",
		app.join(FRAMEWORK_ROOT).join(FRAMEWORK_MAIN),
	)
	.unwrap();
	symlink(
		"Versions/Current/Resources",
		app.join(FRAMEWORK_ROOT).join("Resources"),
	)
	.unwrap();

	let items = build(&app).unwrap();
	let paths = items
		.iter()
		.map(|item| item.relative.as_path())
		.collect::<Vec<_>>();
	assert_eq!(
		paths,
		[FRAMEWORK_A, FRAMEWORK_B, HELPER, PLUGIN, CODELESS, ""].map(Path::new)
	);
	assert_eq!(
		items[0].executable,
		Path::new(FRAMEWORK_A).join(FRAMEWORK_MAIN)
	);
	assert_eq!(
		items[3].executable,
		Path::new(PLUGIN).join(ROOT_BIN).join("Action")
	);
	assert_eq!(items[4].executable, Path::new(CODELESS).join(ROOT_PLIST));
	assert_eq!(items[5].executable, Path::new(ROOT_BIN).join(ROOT_MAIN));
	assert_eq!(
		items.iter().map(|item| item.hardened).collect::<Vec<_>>(),
		[false, false, true, true, false, false]
	);
}

#[test]
fn plans_extension_bundles_instead_of_their_main_files() {
	let temp = tempfile::tempdir().unwrap();
	let app = temp.path().join(APP);
	make_bundle(&app, ROOT_MAIN, SCRIPT);
	make_bundle(&app.join(SYSTEM_EXTENSION), "Network", &macho(false));
	make_bundle(&app.join(DRIVER_EXTENSION), "Device", &macho(false));
	let items = build(&app).unwrap();
	let relative: Vec<_> = items.iter().map(|item| item.relative.as_path()).collect();
	assert_eq!(
		relative,
		[DRIVER_EXTENSION, SYSTEM_EXTENSION, ""].map(Path::new)
	);
	assert_eq!(
		items[0].executable,
		Path::new(DRIVER_EXTENSION).join(ROOT_BIN).join("Device")
	);
	assert_eq!(
		items[1].executable,
		Path::new(SYSTEM_EXTENSION).join(ROOT_BIN).join("Network")
	);
}

#[test]
fn rejects_linked_or_external_bundle_executables() {
	let temp = tempfile::tempdir().unwrap();
	let app = temp.path().join(APP);
	make_bundle(&app, ROOT_MAIN, SCRIPT);
	let executable = app.join(ROOT_BIN).join(ROOT_MAIN);
	let external = temp.path().join("outside");
	fs::hard_link(&executable, &external).unwrap();
	assert!(matches!(build(&app), Err(Error::LinkedExecutable(path)) if path == executable));
	fs::remove_file(&executable).unwrap();
	symlink(&external, &executable).unwrap();
	assert!(matches!(build(&app), Err(Error::Bundle(path)) if path == executable));
}

#[test]
fn rejects_ambiguous_executable_name_or_property_list() {
	let temp = tempfile::tempdir().unwrap();
	let app = temp.path().join(APP);
	make_bundle(&app, ROOT_MAIN, SCRIPT);
	let info = app.join(ROOT_PLIST);
	write_info(&info, Some("../Main"));
	assert!(matches!(build(&app), Err(Error::Bundle(path)) if path == info));
	fs::write(
		&info,
		br#"<plist version="1.0"><dict><key>CFBundleExecutable</key><string>Main</string><key>CFBundleExecutable</key><string>Other</string></dict></plist>"#,
	)
	.unwrap();
	assert!(matches!(build(&app), Err(Error::Bundle(path)) if path == info));
}

fn make_bundle(app: &Path, name: &str, code: &[u8]) {
	write_info(&app.join(ROOT_PLIST), Some(name));
	write_executable(&app.join(ROOT_BIN).join(name), code);
}

fn write_info(path: &Path, name: Option<&str>) {
	let mut info = plist::Dictionary::new();
	info.insert(
		"CFBundleIdentifier".into(),
		"org.dmgez.bundle-fixture".into(),
	);
	if let Some(name) = name {
		info.insert("CFBundleExecutable".into(), name.into());
	}
	fs::create_dir_all(path.parent().unwrap()).unwrap();
	plist::Value::Dictionary(info).to_file_xml(path).unwrap();
}

fn write_executable(path: &Path, code: &[u8]) {
	fs::create_dir_all(path.parent().unwrap()).unwrap();
	fs::write(path, code).unwrap();
	fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn macho(library: bool) -> [u8; 32] {
	const MACH64: u32 = 0xfeed_facf;
	const ARM64: u32 = 0x0100_000c;
	const EXECUTE: u32 = 0x2;
	const DYLIB: u32 = 0x6;
	let mut header = [0; 32];
	header[0..4].copy_from_slice(&MACH64.to_le_bytes());
	header[4..8].copy_from_slice(&ARM64.to_le_bytes());
	header[12..16].copy_from_slice(&(if library { DYLIB } else { EXECUTE }).to_le_bytes());
	header
}
