//! Parser coverage for the schema portion of D-BUILD-01.

use std::{collections::BTreeMap, path::Path};

use dmgez_config::{
	AppSigning, Backend, Background, Capacity, Column, CompressionError, ConfigError, ContentError,
	DefineError, Direction, Filesystem, IconSlot, ImageError, ImageFormat, Payload, Project,
	SigningMode, Size, SizeError, VolumeIcon,
};

const MINIMAL: &str = include_str!("../../../../examples/minimal.toml");
const MULTI_APP: &str = include_str!("../../../../examples/multi-app.toml");
const RELEASE: &str = include_str!("../../../../examples/release.toml");
const HEADER: &str = "schema_version = 1\n[image]\nname = 'Example'\noutput='a.dmg'\n";

#[test]
fn admits_only_complete_positive_compression_cache_settings() {
	assert!(Project::parse(HEADER).unwrap().performance.cache.is_none());
	let cache = "[performance.cache]\ndirectory = 'build-cache'\n";
	for limit in ["1048576", "'1MiB'"] {
		let project = Project::parse(&format!("{HEADER}{cache}max_bytes = {limit}\n")).unwrap();
		let cache = project.performance.cache.unwrap();
		assert_eq!(cache.max_bytes.get(), 1 << 20);
		assert_eq!(cache.directory.as_path(), Path::new("build-cache"));
	}
	for settings in [
		"[performance.cache]\nmax_bytes = '1MiB'\n",
		cache,
		"[performance.cache]\ndirectory = 'https://example.org'\nmax_bytes = '1MiB'\n",
		"[performance.cache]\ndirectory = 'build-cache'\nmax_bytes = '1MiB'\nunknown = true\n",
		"[performance.cache]\ndirectory = 'build-cache'\nmax_bytes = 0\n",
		"[performance.cache]\ndirectory = 'build-cache'\nmax_bytes = '0MiB'\n",
	] {
		assert_eq!(
			Project::parse(&format!("{HEADER}{settings}")).unwrap_err(),
			ConfigError::Schema,
		);
	}
}

#[test]
fn admits_checked_in_examples_without_opening_their_paths() {
	let minimal = Project::parse(MINIMAL).unwrap();
	assert_eq!(minimal.image.format, ImageFormat::Udzo);
	assert_eq!(minimal.image.filesystem, Filesystem::Hfsplus);
	assert_eq!(minimal.image.capacity(), Capacity::Auto { shrink: false });
	assert!(minimal.build.fail_on_metadata_loss(&minimal.metadata));
	assert_eq!(minimal.contents[0].destination.as_str(), "/Example.app");
	let Payload::Copy(copy) = &minimal.contents[0].payload else {
		panic!("expected a copy");
	};
	assert_eq!(
		copy.source.resolve(Path::new("fixture")),
		Path::new("fixture/dist/Example.app")
	);
	let Payload::Symlink { target } = &minimal.contents[1].payload else {
		panic!("expected an explicit symlink");
	};
	assert_eq!(target.as_str(), "/Applications");

	let rich = Project::parse(MULTI_APP).unwrap();
	assert_eq!(rich.image.name.as_str(), "Example Suite 2.0.0");
	assert_eq!(rich.image.capacity(), Capacity::Auto { shrink: true });
	let finder = rich.finder.unwrap();
	let Some(Background::Image { source, retina }) = finder.options.background else {
		panic!("expected an image background");
	};
	assert_eq!(source.as_path(), Path::new("assets/background.png"));
	assert!(retina.is_some());
	let list = finder.options.list_view.unwrap();
	assert_eq!(
		list.sort_directions[&Column::DateModified],
		Direction::Descending
	);
	assert_eq!(list.column_widths[&Column::Name].get(), 300.0);
	assert_eq!(finder.folders[0].path.as_str(), "/Utilities");
	let Some(VolumeIcon::Images(icons)) = rich.volume_icon else {
		panic!("expected explicit icon images");
	};
	assert_eq!(
		icons[&IconSlot::LargestRetina].as_path(),
		Path::new("assets/icon-1024.png")
	);

	let release = Project::parse(RELEASE).unwrap();
	assert_eq!(release.signing.unwrap().apps, Some(AppSigning::Verify));
	assert_eq!(release.notarization.unwrap().staple, Some(true));
	assert_eq!(release.release.unwrap().require_notarization, Some(true));
}

#[test]
fn keeps_developer_id_as_the_legacy_signing_mode() {
	let manifest = format!("{HEADER}[signing]\nenabled=true\ndmg=true\n");
	let signing = Project::parse(&manifest).unwrap().signing.unwrap();
	assert_eq!(signing.mode, SigningMode::DeveloperId);
	assert_eq!(signing.backend.unwrap_or_default(), Backend::Auto);
	assert!(signing.identifier.is_none());
}

#[test]
fn parses_explicit_signing_modes_and_backends() {
	const DISK_IDENTIFIER: &str = "org.pomologie.test-installer";
	for (mode, expected_mode) in [
		("developer-id", SigningMode::DeveloperId),
		("ad-hoc", SigningMode::AdHoc),
	] {
		for (backend, expected_backend) in [
			("auto", Backend::Auto),
			("rust", Backend::Rust),
			("native", Backend::Native),
		] {
			let manifest = format!(
				"{HEADER}[signing]\nenabled=true\ndmg=true\nmode='{mode}'\nbackend='{backend}'\nidentifier='{DISK_IDENTIFIER}'\n"
			);
			let signing = Project::parse(&manifest).unwrap().signing.unwrap();
			assert_eq!(signing.mode, expected_mode);
			assert_eq!(signing.backend, Some(expected_backend));
			assert_eq!(signing.identifier.unwrap().as_str(), DISK_IDENTIFIER);
		}
	}
}

#[test]
fn keeps_draft_format_and_localized_license_fields() {
	let project = Project::parse(&format!(
		"{HEADER}format='ulmo'\nfilesystem='apfs'\n\
		[license]\ndefault_language='en'\n[license.licenses]\nen='license.rtf'\n\
		[license.buttons.en]\nagree='Agree'\n"
	))
	.unwrap();
	assert_eq!(project.image.format, ImageFormat::Ulmo);
	assert_eq!(project.image.filesystem, Filesystem::Apfs);
	let license = project.license.unwrap();
	assert_eq!(license.default_language.as_str(), "en");
	assert_eq!(license.licenses["en"].as_path(), Path::new("license.rtf"));
	assert_eq!(
		license.buttons["en"].agree.as_ref().unwrap().as_str(),
		"Agree"
	);
}

#[test]
fn checks_schema_version_before_definitions_and_model_fields() {
	assert_eq!(
		Project::parse("schema_version = 2\ndefines = 3\nunknown = '${MISSING}'\n").unwrap_err(),
		ConfigError::UnsupportedVersion(2),
	);
	for input in ["", "schema_version = '1'\n", "schema_version = true\n"] {
		assert_eq!(
			Project::parse(input).unwrap_err(),
			ConfigError::SchemaVersion
		);
	}
	assert!(matches!(
		Project::parse("schema_version = ["),
		Err(ConfigError::Syntax { .. })
	));
}

#[test]
fn applies_explicit_defines_as_literal_string_values() {
	let explicit = BTreeMap::from([("VERSION".to_owned(), "2.1.0 ${LITERAL}".to_owned())]);
	let project = Project::parse_with_defines(MULTI_APP, &explicit).unwrap();
	assert_eq!(
		project.image.name.as_str(),
		"Example Suite 2.1.0 ${LITERAL}"
	);
	assert_eq!(
		project.image.output.as_path(),
		Path::new("dist/ExampleSuite-2.1.0 ${LITERAL}.dmg")
	);
	assert_eq!(
		Project::parse(&format!(
			"{HEADER}[[contents]]\nsource = '${{UNDECLARED}}'\n"
		))
		.unwrap_err(),
		ConfigError::Define(DefineError::UnknownVariable),
	);
	assert_eq!(
		Project::parse(&format!("{HEADER}[[contents]]\nsource = '${{UNFINISHED'\n")).unwrap_err(),
		ConfigError::Define(DefineError::Unterminated),
	);
}

#[test]
fn rejects_unknown_fields_in_all_scopes_and_omits_values_from_errors() {
	for section in [
		"unknown='private'",
		"[[contents]]\nsource='file'\nunknown='private'",
		"[finder.icon_view]\nunknown='private'",
		"[[finder.folders]]\npath='/Nested'\nunknown='private'",
		"[finder]\nbackground={kind='image', source='image.png', unknown='private'}",
		"[finder]\nbackground={kind='none', unknown='private'}",
		"[signing]\nunknown='private'",
		"[signing]\nmode='private'",
		"[signing]\nbackend='private'",
		"[signing]\nprivate_key='private'",
	] {
		let error = Project::parse(&format!("{HEADER}{section}\n")).unwrap_err();
		assert!(matches!(error, ConfigError::Schema), "{section}");
		assert!(!format!("{error:?} {error}").contains("private"));
	}
	assert_eq!(
		Project::parse(&format!("{HEADER}format='private'\n")).unwrap_err(),
		ConfigError::Schema,
	);
	assert!(matches!(
		Project::parse(&format!(
			"{HEADER}[finder.list_view]\ncolumns=['name','name']\n"
		)),
		Err(ConfigError::Schema)
	));
}

#[test]
fn finder_visibility_selections_name_distinct_absolute_destinations() {
	let settings = "[finder]\nhide = ['/Kit/Details']\nhide_extensions = ['/Kit/Details', '/Kit/Manual.pdf']\n";
	let finder = Project::parse(&format!("{HEADER}{settings}"))
		.unwrap()
		.finder
		.unwrap();
	assert_eq!(finder.hide[0].as_str(), "/Kit/Details");
	assert_eq!(finder.hide_extensions[1].as_str(), "/Kit/Manual.pdf");
	for settings in [
		"[finder]\nhide = ['/']",
		"[finder]\nhide = ['/Kit/../Details']",
		"[finder]\nhide = ['/Kit/Details', '/Kit/Details']",
		"[finder]\nhide_extensions = ['Kit/Manual.pdf']",
		"[[finder.folders]]\npath = '/Kit'\nhide = ['/Kit/Details']",
	] {
		assert!(
			Project::parse(&format!("{HEADER}{settings}")).is_err(),
			"{settings}"
		);
	}
}

#[test]
fn resolves_capacity_and_rejects_invalid_size_and_compression() {
	let raw = Project::parse(&format!(
		"{HEADER}size='8MiB'\nshrink=true\nfree_space=1024\n"
	))
	.unwrap();
	let Capacity::ShrinkWithin(maximum) = raw.image.capacity() else {
		panic!("expected explicit maximum");
	};
	assert_eq!(maximum.get(), 8 * 1024 * 1024);
	assert_eq!(raw.image.free_space.get(), 1024);
	for size in ["0", "513", "'513B'", "'1k'"] {
		assert_eq!(
			Project::parse(&format!("{HEADER}size={size}\n")).unwrap_err(),
			ConfigError::Image(ImageError::SizeAlignment),
		);
	}
	assert_eq!(
		Project::parse(&format!("{HEADER}size=512\nfree_space=1024")).unwrap_err(),
		ConfigError::Image(ImageError::FreeSpaceExceedsSize),
	);
	assert_eq!(
		Project::parse(&format!(
			"{HEADER}format='udro'\n[compression]\nblock_size=512"
		))
		.unwrap_err(),
		ConfigError::Compression(CompressionError::Format),
	);
	assert_eq!(
		Project::parse(&format!("{HEADER}[compression]\nblock_size='2MiB'")).unwrap_err(),
		ConfigError::Compression(CompressionError::BlockSize),
	);
}

#[test]
fn admits_dmgbuild_capacity_units_with_checked_arithmetic() {
	for (source, expected) in [
		("2048k", 2_048_000),
		("64m", 64_000_000),
		("8g", 8_000_000_000),
		("1t", 1_000_000_000_000),
		("2048000", 2_048_000),
		("4MiB", 4_194_304),
		("2048000B", 2_048_000),
	] {
		let project = Project::parse(&format!("{HEADER}size = '{source}'\n")).unwrap();
		let Capacity::Exact(bytes) = project.image.capacity() else {
			panic!("expected exact capacity for {source:?}");
		};
		assert_eq!(bytes.get(), expected, "{source}");
	}
	for source in [
		"18446744073709552k",
		"18446744073709551616",
		"18446744073709552MiB",
	] {
		assert_eq!(source.parse::<Size>(), Err(SizeError::Overflow), "{source}");
	}
	for source in ["", "-1k", "1K", "1.25m", "2mb", "1,000k", "2 k"] {
		assert_eq!(
			source.parse::<Size>(),
			Err(SizeError::InvalidImage),
			"{source}"
		);
	}
}

#[test]
fn rejects_unsafe_or_conflicting_content_declarations() {
	for (entry, expected) in [
		(
			"type='directory'\ndestination='/'",
			ContentError::RootDestination,
		),
		(
			"source='x'\ndestination='/.ds_store'",
			ContentError::ReservedDestination,
		),
		(
			"type='directory'\ndestination='/Folder'\ninclude=[]",
			ContentError::UnexpectedCopySettings,
		),
		(
			"type='symlink'\ndestination='/link'",
			ContentError::TargetRequired,
		),
		(
			"source='data'\ninclude=['a','a']",
			ContentError::InvalidPatterns,
		),
		(
			"source='data'\nexclude=['../private']",
			ContentError::InvalidPatterns,
		),
		(
			"source='data'\nrecursive=false\nexclude=[]",
			ContentError::NonRecursivePatterns,
		),
	] {
		assert_eq!(
			Project::parse(&format!("{HEADER}[[contents]]\n{entry}\n")).unwrap_err(),
			ConfigError::Content {
				index: 0,
				error: expected
			},
		);
	}
	for path in ["relative", "/a//b", "/a/../b", "/a/", "/a:stream", r"/a\b"] {
		assert!(matches!(
			Project::parse(&format!(
				"{HEADER}[[contents]]\nsource='data'\ndestination='{path}'\n"
			)),
			Err(ConfigError::Schema),
		));
	}
	let parent =
		format!("{HEADER}[[contents]]\ntype='symlink'\ndestination='/Parent'\ntarget='external'");
	assert_eq!(
		Project::parse(&format!(
			"{parent}\n[[contents]]\nsource='copy'\ndestination='/Parent/nested'\n"
		))
		.unwrap_err(),
		ConfigError::Content {
			index: 1,
			error: ContentError::SymlinkAncestor { other: 0 }
		},
	);
	assert_eq!(
		Project::parse(&format!(
			"{parent}\n[[contents]]\nsource='copy'\ndestination='/Parent'\n"
		))
		.unwrap_err(),
		ConfigError::Content {
			index: 1,
			error: ContentError::DuplicateDestination { other: 0 }
		},
	);
}
