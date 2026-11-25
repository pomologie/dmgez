use std::fs;

use super::*;

const HELPER: &str = "Contents/Helpers/Helper";
const ENTITLEMENT: &str = "com.apple.security.cs.allow-jit";

fn plan(path: &str) -> Item {
	Item {
		relative: PathBuf::from(path),
		executable: PathBuf::from("Contents/MacOS/Test"),
		hardened: true,
	}
}

fn xml(contents: &str) -> String {
	format!("<plist version=\"1.0\"><dict>{contents}</dict></plist>")
}

#[test]
fn matches_only_exact_code_item_paths_and_admits_dot_for_the_root() {
	let directory = tempfile::tempdir().unwrap();
	let policy = directory.path().join("input.plist");
	fs::write(&policy, xml(&format!("<key>{ENTITLEMENT}</key><true/>"))).unwrap();
	let items = [plan(HELPER), plan("")];
	let rule = |item: &'static str| AppEntitlements {
		item: Path::new(item),
		plist: &policy,
	};
	let staged = prepare(&[rule("."), rule(HELPER)], &items, directory.path()).unwrap();
	assert!(staged.path(Path::new("")).unwrap().is_file());
	assert!(staged.path(Path::new(HELPER)).unwrap().is_file());
	assert!(matches!(
		prepare(&[rule("Contents/helpers/Helper")], &items, directory.path()),
		Err(Error::UnmatchedItem(_))
	));
	assert!(matches!(
		prepare(
			&[rule("Contents//Helpers/Helper")],
			&items,
			directory.path()
		),
		Err(Error::InvalidItem(_))
	));
	assert!(matches!(
		prepare(&[rule("."), rule(".")], &items, directory.path()),
		Err(Error::DuplicateItem(_))
	));
}

#[test]
fn rejects_debugging_entitlements_and_invalid_dictionary_roles() {
	let directory = tempfile::tempdir().unwrap();
	let policy = directory.path().join("input.plist");
	let rule = AppEntitlements {
		item: Path::new("."),
		plist: &policy,
	};
	let body = format!("<key>{GET_TASK_ALLOW}</key><true/>");
	fs::write(&policy, xml(&body)).unwrap();
	assert!(matches!(
		prepare(&[rule], &[plan("")], directory.path()),
		Err(Error::DebuggingEnabled(_))
	));
	for body in [
		"<key>one</key><true/><key>one</key><false/>",
		"<string>one</string><true/>",
		"<key>one</key><dict><key>nested</key><true/><key>nested</key><false/></dict>",
	] {
		assert!(parse_dictionary(xml(body).as_bytes()).is_none(), "{body}");
	}
}

#[test]
fn verification_distinguishes_absent_explicit_empty_and_expected_entitlements() {
	let allowed = xml(&format!("<key>{ENTITLEMENT}</key><true/>"));
	let expected = parse_dictionary(allowed.as_bytes()).unwrap();
	assert!(matches_entitlements(Some(&expected), allowed.as_bytes()));
	assert!(!matches_entitlements(None, allowed.as_bytes()));
	assert!(!matches_entitlements(Some(&expected), b""));
	assert!(matches_entitlements(None, b""));
	assert!(!matches_entitlements(None, xml("").as_bytes()));
}

#[test]
fn profile_must_be_a_bounded_regular_file() {
	let directory = tempfile::tempdir().unwrap();
	let source = directory.path().join("policy.plist");
	let link = directory.path().join("linked.plist");
	let item = Path::new(".");
	fs::write(&source, xml("")).unwrap();
	std::os::unix::fs::symlink(&source, &link).unwrap();
	assert!(matches!(
		prepare(
			&[AppEntitlements { item, plist: &link }],
			&[plan("")],
			directory.path()
		),
		Err(Error::InvalidFile(_))
	));
	fs::File::options()
		.write(true)
		.open(&source)
		.unwrap()
		.set_len(MAX_PLIST_BYTES as u64 + 1)
		.unwrap();
	assert!(matches!(
		prepare(
			&[AppEntitlements {
				item,
				plist: &source
			}],
			&[plan("")],
			directory.path()
		),
		Err(Error::InvalidFile(_))
	));
}

#[test]
fn native_round_trip_requires_the_exact_dictionary_or_no_entitlements() {
	const SIGN_ARGS: &[&str] = &["--sign", "-", "--force", "--timestamp=none"];
	const ENTITLEMENTS_OPTION: &str = "--entitlements";
	let directory = tempfile::tempdir().unwrap();
	let target = directory.path().join("Helper");
	fs::copy(std::env::current_exe().unwrap(), &target).unwrap();
	let policy = directory.path().join("input.plist");
	fs::write(&policy, xml(&format!("<key>{ENTITLEMENT}</key><true/>"))).unwrap();
	let rule = AppEntitlements {
		item: Path::new("."),
		plist: &policy,
	};
	let relative = Path::new("");
	let staged = prepare(&[rule], &[plan("")], directory.path()).unwrap();
	let unruled = prepare(&[], &[plan("")], directory.path()).unwrap();
	let architecture = if cfg!(target_arch = "aarch64") {
		"arm64"
	} else {
		"x86_64"
	};

	let sign = |entitlements: Option<&Path>| {
		let mut args: Vec<_> = SIGN_ARGS.iter().map(OsStr::new).collect();
		if let Some(plist) = entitlements {
			args.extend([OsStr::new(ENTITLEMENTS_OPTION), plist.as_os_str()]);
		}
		args.extend([OsStr::new(END_OPTIONS), target.as_os_str()]);
		assert!(runner::codesign(&args).unwrap().status.success());
	};
	sign(staged.path(relative));
	staged
		.verify(relative, &target, Some(architecture))
		.unwrap();
	assert!(unruled.verify(relative, &target, None).is_err());

	sign(None);
	unruled
		.verify(relative, &target, Some(architecture))
		.unwrap();
	assert!(staged.verify(relative, &target, None).is_err());
}
