//! Typed override coverage for D-BUILD-01-T03.

use std::{collections::BTreeMap, path::Path};

use dmgez_config::{
	AutoFlag, Capacity, CompressionError, ConfigError, ConfigOverride, DefineError, ImageError,
	ImageFormat, MetadataPolicy, OverrideError, Payload, Project,
};

const BASE: &str = "schema_version = 1\n[image]\nname = 'Example'\noutput = 'example.dmg'\n";
const SECRET: &str = "synthetic-private-value";

fn parse(input: &str, assignments: &[&str]) -> Result<Project, ConfigError> {
	let assignments = assignments
		.iter()
		.map(|argument| ConfigOverride::parse(argument))
		.collect::<Result<Vec<_>, _>>()?;
	Project::parse_with_overrides(input, &BTreeMap::new(), &assignments, Path::new(""))
}

#[test]
fn definitions_apply_to_manifest_and_scalar_overrides_win_in_order() {
	const INPUT: &str = "schema_version = 1\n[image]\nname = '${CHANNEL}'\noutput = 'dist/${CHANNEL}.dmg'\n[defines]\nCHANNEL = 'development'\n[compression]\nlevel = 2\n";
	const NAME: &str = "Product ${HOME} $(printf literal) build=stable";
	let definitions = BTreeMap::from([("CHANNEL".to_owned(), "release".to_owned())]);
	let overrides = [
		ConfigOverride::parse("image.name=Earlier").unwrap(),
		ConfigOverride::parse(&format!("image.name={NAME}")).unwrap(),
		ConfigOverride::parse("compression.level=4").unwrap(),
		ConfigOverride::parse("performance.parallel=false").unwrap(),
		ConfigOverride::parse("image.size=8MiB").unwrap(),
		ConfigOverride::parse("image.shrink=true").unwrap(),
		ConfigOverride::parse("image.free_space=1024").unwrap(),
		ConfigOverride::parse("metadata.policy=report").unwrap(),
		ConfigOverride::parse("performance.max_entries=23").unwrap(),
	];
	let project = Project::parse_with_overrides(INPUT, &definitions, &overrides, Path::new(""))
		.expect("build settings should admit the overrides");
	assert_eq!(project.image.name.as_str(), NAME);
	assert_eq!(
		project.image.output.as_path(),
		Path::new("dist/release.dmg")
	);
	assert_eq!(project.image.format, ImageFormat::Udzo);
	assert_eq!(project.compression.level, Some(4));
	assert_eq!(project.performance.parallel, AutoFlag::Disabled);
	assert_eq!(project.image.free_space.get(), 1024);
	let Capacity::ShrinkWithin(maximum) = project.image.capacity() else {
		panic!("expected an explicit shrink limit");
	};
	assert_eq!(maximum.get(), 8 << 20);
	assert_eq!(project.metadata.policy, MetadataPolicy::Report);
	assert_eq!(project.performance.max_entries.unwrap().get(), 23);
}

#[test]
fn accepts_decimal_image_capacity_overrides_only_for_the_image_size() {
	let project = parse(BASE, &["image.size=2048k", "image.shrink=true"]).unwrap();
	let Capacity::ShrinkWithin(maximum) = project.image.capacity() else {
		panic!("expected an explicit shrink limit");
	};
	assert_eq!(maximum.get(), 2_048_000);
	assert_eq!(
		parse(BASE, &["performance.memory_limit=64m"]).unwrap_err(),
		ConfigError::Override(OverrideError::Value {
			key: "performance.memory_limit"
		})
	);
}

#[test]
fn only_path_overrides_resolve_against_the_explicit_working_directory() {
	let input = format!("{BASE}[[contents]]\nsource = 'input/fixture.app'\n");
	let working_directory = std::env::temp_dir().join("dmgez-override-caller");
	let overrides = [
		ConfigOverride::parse("image.output=artifacts/build = release.dmg").unwrap(),
		ConfigOverride::parse("metadata.sidecar=meta/manifest.json").unwrap(),
	];
	let project =
		Project::parse_with_overrides(&input, &BTreeMap::new(), &overrides, &working_directory)
			.unwrap();
	assert_eq!(
		project.image.output.as_path(),
		working_directory.join("artifacts/build = release.dmg")
	);
	assert_eq!(
		project.metadata.sidecar.unwrap().as_path(),
		working_directory.join("meta/manifest.json")
	);
	let Payload::Copy(copy) = &project.contents[0].payload else {
		panic!("expected a manifest source");
	};
	assert_eq!(copy.source.as_path(), Path::new("input/fixture.app"));
	assert_eq!(
		parse(BASE, &["image.output=relative.dmg"]).unwrap_err(),
		ConfigError::Override(OverrideError::WorkingDirectory {
			key: "image.output"
		})
	);
}

#[test]
fn validates_effective_settings_and_preserves_strict_manifest_admission() {
	assert_eq!(
		parse(BASE, &["image.size=513"]).unwrap_err(),
		ConfigError::Image(ImageError::SizeAlignment)
	);
	assert_eq!(
		parse(BASE, &["compression.level=10"]).unwrap_err(),
		ConfigError::Compression(CompressionError::Level)
	);
	assert_eq!(
		parse(BASE, &["image.format=udro", "compression.level=1"]).unwrap_err(),
		ConfigError::Compression(CompressionError::Format)
	);
	assert_eq!(
		parse(&format!("{BASE}size=512\n"), &["image.free_space=1KiB"]).unwrap_err(),
		ConfigError::Image(ImageError::FreeSpaceExceedsSize)
	);
	assert_eq!(
		parse(&format!("{BASE}unknown=true\n"), &["image.name=Other"]).unwrap_err(),
		ConfigError::Schema
	);
	assert_eq!(
		parse(
			&format!("{BASE}[compression]\nlevel={{ unknown=1 }}\n"),
			&["compression.level=1"]
		)
		.unwrap_err(),
		ConfigError::Schema
	);
	assert_eq!(
		parse("schema_version=2", &["image.name=Other"]).unwrap_err(),
		ConfigError::UnsupportedVersion(2)
	);
	assert_eq!(
		parse(
			&format!("{BASE}format='${{MISSING}}'\n"),
			&["image.format=udro"]
		)
		.unwrap_err(),
		ConfigError::Define(DefineError::UnknownVariable)
	);
}

#[test]
fn rejects_unlisted_keys_and_invalid_scalar_types_with_redacted_diagnostics() {
	for argument in [
		format!("{SECRET}={SECRET}"),
		format!("schema_version={SECRET}"),
		format!("defines.SERVICE={SECRET}"),
		format!("contents.0.source={SECRET}"),
		format!("notarization.secret={SECRET}"),
		format!("build.overwrite={SECRET}"),
		format!("image.format={SECRET}"),
		format!("image.output=https://example.com/{SECRET}"),
		format!("performance.threads=$({SECRET})"),
	] {
		let error = ConfigOverride::parse(&argument).unwrap_err();
		assert!(!format!("{error:?} {error}").contains(SECRET));
	}
	assert_eq!(
		ConfigOverride::parse("image.name").unwrap_err(),
		OverrideError::Assignment
	);
	assert_eq!(
		ConfigOverride::parse("performance.max_entries=0").unwrap_err(),
		OverrideError::Value {
			key: "performance.max_entries"
		}
	);
	assert_eq!(
		ConfigOverride::parse("compression.level=256").unwrap_err(),
		OverrideError::Value {
			key: "compression.level"
		}
	);
	let assignment = ConfigOverride::parse(&format!("image.name={SECRET}")).unwrap();
	assert_eq!(assignment.key(), "image.name");
	assert!(!format!("{assignment:?}").contains(SECRET));
}
