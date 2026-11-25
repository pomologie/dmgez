#![cfg(feature = "appdmg-import")]

use std::path::{Path, PathBuf};

use dmgez_config::{
	AppDmgDiagnostic, AppDmgImportError, Background, Filesystem, ImageFormat, MAX_MANIFEST_BYTES,
	Payload, Project, VolumeIcon, import_appdmg,
};

const OUTPUT: &str = "dist/Example.dmg";
const MINIMAL: &str = r#"{"title":"Example","contents":[]}"#;

fn source_directory() -> &'static Path {
	if cfg!(windows) {
		Path::new(r"C:\not-present\project")
	} else {
		Path::new("/not-present/project")
	}
}

#[test]
fn imports_installer_layout_and_paths_without_opening_sources() {
	let json = r##"{
		"title": "Example ${NAME}",
		"icon": "assets/icon.icns",
		"background": "assets/paper.png",
		"background-color": "#abcdef",
		"icon-size": 96.0,
		"window": {
			"position": {"x": -3, "y": 80},
			"size": {"width": 720, "height": 360}
		},
		"format": "UDRO",
		"filesystem": "APFS",
		"contents": [
			{"x": 192, "y": 100, "type": "file", "path": "inputs/../Example ${NAME}.app", "name": "Example.app"},
			{"x": 448, "y": 344, "type": "link", "path": "/Applications"},
			{"x": 200, "y": 300, "type": "position", "path": "Example.app"}
		]
	}"##;
	let root = source_directory();
	let imported = import_appdmg(json, root, OUTPUT).unwrap();
	assert_eq!(
		imported.diagnostics,
		[
			AppDmgDiagnostic::RetinaBackground,
			AppDmgDiagnostic::BackgroundColorShadowed
		]
	);
	let project = Project::parse(&imported.manifest).unwrap();
	assert_eq!(project.image.name.as_str(), "Example ${NAME}");
	assert_eq!(project.image.output.as_path(), Path::new(OUTPUT));
	assert_eq!(project.image.format, ImageFormat::Udro);
	assert_eq!(project.image.filesystem, Filesystem::Apfs);
	assert_eq!(project.compression.level, None);
	let Some(VolumeIcon::Source(icon)) = project.volume_icon else {
		panic!("expected an icon source")
	};
	assert_eq!(icon.as_path(), root.join("assets/icon.icns"));
	let finder = project.finder.unwrap().options;
	assert_eq!(
		finder.window_position.unwrap().map(|part| part.get()),
		[-3.0, 80.0]
	);
	assert_eq!(
		finder.window_size.unwrap().map(|part| part.get()),
		[720.0, 360.0]
	);
	assert_eq!(finder.icon_view.unwrap().icon_size.unwrap().get(), 96.0);
	let Some(Background::Image {
		source,
		retina: None,
	}) = finder.background
	else {
		panic!("expected a single-image background")
	};
	assert_eq!(source.as_path(), root.join("assets/paper.png"));
	let [app, applications] = &project.contents[..] else {
		panic!("expected two entries")
	};
	let Payload::Copy(copy) = &app.payload else {
		panic!("expected a copy")
	};
	assert_eq!(app.destination.as_str(), "/Example.app");
	assert_eq!(copy.source.as_path(), root.join("Example ${NAME}.app"));
	assert!(copy.recursive);
	assert_eq!(app.position.unwrap().map(|part| part.get()), [200.0, 300.0]);
	assert_eq!(applications.destination.as_str(), "/Applications");
	let Payload::Symlink { target } = &applications.payload else {
		panic!("expected a symlink")
	};
	assert_eq!(target.as_str(), "/Applications");
}

#[test]
fn preserves_defaults_and_reports_window_sizing_that_needs_image_access() {
	let root = source_directory();
	let color = r##"{"title":"Example","contents":[],"background-color":"#0055aa"}"##;
	let result = import_appdmg(color, root, OUTPUT).unwrap();
	assert!(result.diagnostics.is_empty());
	let project = Project::parse(&result.manifest).unwrap();
	assert_eq!(project.image.format, ImageFormat::Udzo);
	assert_eq!(project.image.filesystem, Filesystem::Hfsplus);
	assert_eq!(project.compression.level, Some(9));
	let options = project.finder.unwrap().options;
	assert_eq!(options.icon_view.unwrap().icon_size.unwrap().get(), 80.0);
	assert_eq!(
		options.window_size.unwrap().map(|part| part.get()),
		[640.0, 480.0]
	);
	let Some(Background::Color { value }) = options.background else {
		panic!("expected a color")
	};
	assert_eq!(value.as_str(), "#0055aa");

	let image = r#"{"title":"Example","contents":[],"background":"missing.png"}"#;
	let result = import_appdmg(image, root, OUTPUT).unwrap();
	assert_eq!(
		result.diagnostics,
		[
			AppDmgDiagnostic::RetinaBackground,
			AppDmgDiagnostic::BackgroundWindowSize
		]
	);
	let project = Project::parse(&result.manifest).unwrap();
	assert!(project.finder.unwrap().options.window_size.is_none());
}

#[test]
fn retains_each_documented_format_as_project_intent() {
	for (external, expected) in [
		("UDRW", ImageFormat::Udrw),
		("UDRO", ImageFormat::Udro),
		("UDCO", ImageFormat::Udco),
		("UDZO", ImageFormat::Udzo),
		("UDBZ", ImageFormat::Udbz),
		("ULFO", ImageFormat::Ulfo),
		("ULMO", ImageFormat::Ulmo),
	] {
		let json = format!(r#"{{"title":"Example","contents":[],"format":"{external}"}}"#);
		let imported = import_appdmg(&json, source_directory(), OUTPUT).unwrap();
		let project = Project::parse(&imported.manifest).unwrap();
		assert_eq!(project.image.format, expected);
		assert_eq!(
			project.compression.level,
			(expected == ImageFormat::Udzo).then_some(9)
		);
	}
}

#[test]
fn rejects_unknown_fields_duplicate_keys_and_non_schema_shapes() {
	let root = source_directory();
	for json in [
		r#"["Example",[]]"#,
		r#"{"title":"Example","contents":[],"typo":true}"#,
		r#"{"title":"Example","contents":[],"title":"Another"}"#,
		r#"{"title":"Example","contents":[],"icon":null}"#,
		r#"{"title":"Example","contents":[],"window":{"scale":2}}"#,
		r#"{"title":"Example","contents":[],"window":{"position":[3,4]}}"#,
		r#"{"title":"Example","contents":[],"window":{"position":{"x":1.5,"y":2}}}"#,
		r#"{"title":"Example","contents":[],"format":"raw"}"#,
		r#"{"title":"Example","contents":[[1,2,"file","file.txt"]]}"#,
		r#"{"title":"Example","contents":[{"x":3,"y":4,"path":"abc","type":"exec"}]}"#,
	] {
		assert!(
			matches!(
				import_appdmg(json, root, OUTPUT),
				Err(AppDmgImportError::Json(_))
			),
			"accepted {json}"
		);
	}
}

#[test]
fn rejects_untranslatable_fields_and_bounds_input() {
	let root = source_directory();
	assert!(matches!(
		import_appdmg(MINIMAL, Path::new("relative"), OUTPUT),
		Err(AppDmgImportError::SourceDirectory)
	));
	let signed = r#"{"title":"Example","contents":[],"code-sign":{"signing-identity":"Example"}}"#;
	assert!(matches!(
		import_appdmg(signed, root, OUTPUT),
		Err(AppDmgImportError::Signing)
	));
	let position =
		r#"{"title":"Example","contents":[{"x":3,"y":4,"path":"absent.txt","type":"position"}]}"#;
	assert!(matches!(
		import_appdmg(position, root, OUTPUT),
		Err(AppDmgImportError::PositionTarget { index: 0 })
	));
	let escape = r#"{"title":"Example","contents":[{"x":3,"y":4,"path":"file.txt","name":"../escape","type":"file"}]}"#;
	assert!(matches!(
		import_appdmg(escape, root, OUTPUT),
		Err(AppDmgImportError::Destination { index: 0 })
	));
	let remote = r#"{"title":"Example","contents":[],"icon":"https://example.com/icon.icns"}"#;
	assert!(matches!(
		import_appdmg(remote, root, OUTPUT),
		Err(AppDmgImportError::SourcePath)
	));
	let oversized = " ".repeat(MAX_MANIFEST_BYTES + 1);
	assert!(matches!(
		import_appdmg(&oversized, root, OUTPUT),
		Err(AppDmgImportError::InputLimit)
	));
	let long_directory: PathBuf = root.join("a".repeat(MAX_MANIFEST_BYTES));
	assert!(matches!(
		import_appdmg(MINIMAL, &long_directory, OUTPUT),
		Err(AppDmgImportError::OutputLimit)
	));
}
