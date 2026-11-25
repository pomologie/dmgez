use std::{ffi::OsStr, fs, path::Path};

use super::*;
use crate::native::{END_OPTIONS, policy, runner};

const SOURCE_NAME: &str = "Source.app";
const OUTPUT_NAME: &str = "Signed.app";
const HELPER: &str = "Contents/Helpers/Helper.app";
const MAIN_EXECUTABLE: &str = "Contents/MacOS/Executable";
const INFO: &str = "Contents/Info.plist";
const RESOURCE: &str = "Contents/Resources/Payload";
const RESOURCE_BYTES: &[u8] = b"synthetic resource\n";
const PROFILE: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict><key>com.apple.security.cs.allow-jit</key><true/></dict>
</plist>
"#;

#[test]
fn signs_nested_items_before_the_root_and_preserves_the_signed_source() {
	let directory = tempfile::tempdir().unwrap();
	let source = directory.path().join(SOURCE_NAME);
	let output = directory.path().join(OUTPUT_NAME);
	let helper = source.join(HELPER);
	make_bundle(&source, "org.dmgez.fixture.parent");
	make_bundle(&helper, "org.dmgez.fixture.parent.helper");
	fixture_sign(&helper, true, None).unwrap();
	fixture_sign(&source, true, None).unwrap();
	fixture_verify(&source, &source.join(MAIN_EXECUTABLE), &[], true, true).unwrap();
	let original = fs::read(source.join(MAIN_EXECUTABLE)).unwrap();
	let original_helper = fs::read(helper.join(MAIN_EXECUTABLE)).unwrap();
	let profile = directory.path().join("Helper entitlements.plist");
	fs::write(&profile, PROFILE).unwrap();
	let rules = [AppEntitlements {
		item: Path::new(HELPER),
		plist: &profile,
	}];
	let result = stage_and_sign(
		&open_app(&source).unwrap(),
		&output,
		&rules,
		|| Ok(()),
		fixture_sign,
		fixture_verify,
	)
	.unwrap();
	assert_eq!(result.path, output.canonicalize().unwrap());
	assert_eq!(result.items.len(), 2);
	assert_eq!(result.items[0].item, HELPER);
	assert!(result.items[0].entitlements);
	assert!(result.items[0].hardened_runtime);
	assert_eq!(result.items[1].item, ROOT_ITEM);
	assert!(!result.items[1].entitlements);
	assert!(result.items[1].hardened_runtime);
	assert_eq!(fs::read(source.join(MAIN_EXECUTABLE)).unwrap(), original);
	assert_eq!(
		fs::read(helper.join(MAIN_EXECUTABLE)).unwrap(),
		original_helper
	);
	assert_eq!(fs::read(output.join(RESOURCE)).unwrap(), RESOURCE_BYTES);
	fixture_verify(&source, &source.join(MAIN_EXECUTABLE), &[], true, true).unwrap();
	fixture_verify(&output, &output.join(MAIN_EXECUTABLE), &[], true, true).unwrap();
	assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 3);
}

#[test]
fn keeps_a_failed_private_signature_out_of_the_destination() {
	let directory = tempfile::tempdir().unwrap();
	let source = directory.path().join(SOURCE_NAME);
	let output = directory.path().join(OUTPUT_NAME);
	make_bundle(&source, "org.dmgez.fixture.failure");
	let before = fs::read(source.join(MAIN_EXECUTABLE)).unwrap();
	let result = stage_and_sign(
		&open_app(&source).unwrap(),
		&output,
		&[],
		|| Ok(()),
		fixture_sign,
		|_, _, _, _, _| Err(SignError::SigningPolicy),
	);
	assert!(matches!(result, Err(SignError::SigningPolicy)));
	assert!(!output.exists());
	assert_eq!(fs::read(source.join(MAIN_EXECUTABLE)).unwrap(), before);
	assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
fn existing_application_verification_rejects_a_valid_ad_hoc_signature() {
	let directory = tempfile::tempdir().unwrap();
	let source = directory.path().join(SOURCE_NAME);
	make_bundle(&source, "org.dmgez.fixture.ad-hoc");
	fixture_sign(&source, true, None).unwrap();
	fixture_verify(&source, &source.join(MAIN_EXECUTABLE), &[], true, true).unwrap();

	assert!(matches!(
		crate::verify_application(&source, crate::Backend::Native),
		Err(SignError::SigningPolicy)
	));
	assert_eq!(fs::read(source.join(RESOURCE)).unwrap(), RESOURCE_BYTES);
}

#[test]
fn existing_application_verification_checks_nested_code_and_revalidates_its_tree() {
	let directory = tempfile::tempdir().unwrap();
	let source = directory.path().join(SOURCE_NAME);
	let helper = source.join(HELPER);
	make_bundle(&source, "org.dmgez.fixture.verify");
	make_bundle(&helper, "org.dmgez.fixture.verify.helper");
	let source = open_app(&source).unwrap();

	let mut checked = Vec::new();
	verify_existing(
		&source,
		|bundle, executable, architectures, runtime, deep| {
			assert_eq!(executable, bundle.join(MAIN_EXECUTABLE));
			assert!(!architectures.is_empty());
			assert!(runtime);
			assert_eq!(deep, bundle == source.path());
			checked.push(bundle.strip_prefix(source.path()).unwrap().to_path_buf());
			Ok(())
		},
	)
	.unwrap();
	assert_eq!(checked, [PathBuf::from(HELPER), PathBuf::new()]);

	let nested = Path::new(HELPER).join(RESOURCE);
	let result = verify_existing(&source, |_, _, _, _, deep| {
		if deep {
			fs::write(source.path().join(&nested), b"changed nested resource").unwrap();
		}
		Ok(())
	});
	let Err(SignError::AppCopy(error)) = result else {
		panic!("bundle verification accepted a changed nested resource");
	};
	assert!(matches!(
		error.downcast_ref(),
		Some(copy::CopyError::SourceChanged(path)) if path == &nested
	));
}

fn make_bundle(path: &Path, id: &str) {
	let executable = path.join(MAIN_EXECUTABLE);
	let resource = path.join(RESOURCE);
	fs::create_dir_all(executable.parent().unwrap()).unwrap();
	fs::create_dir_all(resource.parent().unwrap()).unwrap();
	fs::copy(std::env::current_exe().unwrap(), executable).unwrap();
	fs::write(resource, RESOURCE_BYTES).unwrap();
	fs::write(
		path.join(INFO),
		format!(
			r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleIdentifier</key><string>{id}</string>
<key>CFBundleExecutable</key><string>Executable</string>
</dict></plist>"#,
		),
	)
	.unwrap();
}

fn fixture_sign(path: &Path, hardened: bool, profile: Option<&Path>) -> Result<(), SignError> {
	let options = ["--force", "--sign", "-", "--timestamp=none"];
	let mut args: Vec<_> = options.iter().map(OsStr::new).collect();
	if hardened {
		args.extend(["--options", "runtime"].map(OsStr::new));
	}
	if let Some(profile) = profile {
		args.extend([OsStr::new("--entitlements"), profile.as_os_str()]);
	}
	args.extend([OsStr::new(END_OPTIONS), path.as_os_str()]);
	let result = runner::codesign(&args).unwrap();
	assert!(
		result.status.success(),
		"{}",
		String::from_utf8_lossy(&result.stderr)
	);
	Ok(())
}

fn fixture_verify(
	path: &Path,
	_executable: &Path,
	architectures: &[&str],
	hardened: bool,
	deep: bool,
) -> Result<(), SignError> {
	let options = ["--verify", "--strict", "--all-architectures"];
	let mut args: Vec<_> = options.iter().map(OsStr::new).collect();
	if deep {
		args.push(OsStr::new("--deep"));
	}
	args.extend([OsStr::new(END_OPTIONS), path.as_os_str()]);
	let result = runner::codesign(&args).unwrap();
	assert!(
		result.status.success(),
		"{}",
		String::from_utf8_lossy(&result.stderr)
	);
	if hardened {
		let selected = architectures.iter().copied().map(Some);
		for architecture in selected.chain(architectures.is_empty().then_some(None)) {
			let mut args = ["--display", "--verbose=4"].map(OsStr::new).to_vec();
			if let Some(architecture) = architecture {
				args.extend(["--architecture", architecture].map(OsStr::new));
			}
			args.extend([OsStr::new(END_OPTIONS), path.as_os_str()]);
			let output = runner::codesign(&args).unwrap();
			let evidence = String::from_utf8_lossy(&output.stderr);
			assert!(output.status.success(), "{evidence}");
			assert!(policy::has_hardened_runtime(&evidence), "{evidence}");
		}
	}
	Ok(())
}
