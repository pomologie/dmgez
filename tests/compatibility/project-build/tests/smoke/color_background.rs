use dmgez_builder::{planning, preflight::PreflightError};
use hfsplus_format::volume::CatalogId;

use super::{
	BACKGROUND_FOLDER, Command, Error, FileSource, Header, IMAGE, Image, Node, OutputIntent,
	Project, ROOT_ID, Stdio, Write, catalog_records, first_leaf, folder, fs, hfsplus, open_output,
	options, publish_project, read_fork, store_fork,
};
use crate::PROJECT_HEADER;

const ORACLE: &str = include_str!("color_background_oracle.py");
const BYTE_MAX: f64 = u8::MAX as f64;
const GOLDENROD: [f64; 3] = [218.0 / BYTE_MAX, 165.0 / BYTE_MAX, 32.0 / BYTE_MAX];
const CYAN: [f64; 3] = [0.0, 1.0, 1.0];

// The overlap inputs and RGB triples were checked against dmgbuild 2.0.0.
// The parser is in src/dmgbuild/colors.py at its release commit:
// https://github.com/dmgbuild/dmgbuild/blob/7f80e3e44ea950e27d4fdf66628767dc3396a052/src/dmgbuild/colors.py
const REFERENCE_COLORS: &[(&str, [f64; 3])] = &[
	("goldenrod", GOLDENROD),
	("#246", [34.0 / BYTE_MAX, 68.0 / BYTE_MAX, 102.0 / BYTE_MAX]),
	(
		"#227799",
		[34.0 / BYTE_MAX, 119.0 / BYTE_MAX, 153.0 / BYTE_MAX],
	),
	("rgb(.23,.67,.91)", [0.23, 0.67, 0.91]),
	("hsl(120.0deg,1.0,.5)", [0.0, 1.0, 0.0]),
	("hsl(1.5707963267948966rad,.5,.4)", [0.4, 0.6, 0.2]),
	("hsl(100.0grad,1.0,.5)", [0.5, 1.0, 0.0]),
	("hwb(300.0deg,.15,.25)", [0.75, 0.15, 0.75]),
	("hwb(200.0gon,.1,.2)", [0.1, 0.8, 0.8]),
	("cmyk(.85,.25,.1,.4)", [0.0, 0.35, 0.5]),
	("gray(.425)", [0.425; 3]),
	("grey(.65)", [0.65; 3]),
];

// The documented integer, percentage, and unitless hue forms and the CSS
// normalization of overfull HWB also have independent expected triples.
const DOCUMENTED_COLORS: &[(&str, [f64; 3])] = &[
	("rgb(0, 50%, 1)", [0.0, 0.5, 1.0]),
	("hsl(180,100%,50%)", CYAN),
	("hwb(120,80%,40%)", [2.0 / 3.0; 3]),
	("grey", [128.0 / BYTE_MAX; 3]),
];

fn css_background(value: &str) -> String {
	format!(r#"background = {{ kind = "color", value = "{value}" }}"#)
}

fn assert_color_record(
	image: &mut Image<FileSource>,
	header: &Header,
	node: &Node<'_>,
	parent: CatalogId,
	label: &str,
	rgb: [f64; 3],
) {
	let data = store_fork(node, header, parent);
	let bytes = read_fork(image, header, &data);
	let mut oracle = Command::new("python3")
		.args(["-B", "-c", ORACLE, label])
		.args(rgb.map(|channel| channel.to_string()))
		.stdin(Stdio::piped())
		.spawn()
		.unwrap();
	oracle.stdin.take().unwrap().write_all(&bytes).unwrap();
	assert!(
		oracle.wait().unwrap().success(),
		"oracle failed for {label}"
	);
}

#[test]
#[ignore = "requires Python ds_store 1.3.4"]
fn published_colors_match_the_finder_rgb_record() {
	for &(value, rgb) in REFERENCE_COLORS.iter().chain(DOCUMENTED_COLORS) {
		let temp = tempfile::tempdir().unwrap();
		let project = Project::parse(&format!(
			"{PROJECT_HEADER}\n[finder]\n{}\n",
			css_background(value)
		))
		.unwrap();
		publish_project(temp.path(), &project);
		let (mut image, header) = open_output(&temp.path().join(IMAGE));
		let catalog = first_leaf(&mut image, &header.forks.catalog, header.geometry);
		let node = Node::decode(&catalog).unwrap();
		assert!(!catalog_records(&node, &header).any(|record| {
			record.key.parent_or_thread == ROOT_ID
				&& record.key.name.units().eq(BACKGROUND_FOLDER.encode_utf16())
		}));
		assert_color_record(&mut image, &header, &node, ROOT_ID, value, rgb);
	}
}

#[test]
#[ignore = "requires Python ds_store 1.3.4"]
fn published_folders_inherit_and_override_the_color_record() {
	const INHERITED: &str = "Inherited";
	const OVERRIDE: &str = "Override";
	let temp = tempfile::tempdir().unwrap();
	let project = Project::parse(&format!(
		r#"{PROJECT_HEADER}
[[contents]]
type = "directory"
destination = "/{INHERITED}"

[[contents]]
type = "directory"
destination = "/{OVERRIDE}"

[finder]
{}

[[finder.folders]]
path = "/{INHERITED}"
window_size = [550, 270]

[[finder.folders]]
path = "/{OVERRIDE}"
{}
"#,
		css_background("goldenrod"),
		css_background("hsl(180,100%,50%)"),
	))
	.unwrap();
	publish_project(temp.path(), &project);
	let (mut image, header) = open_output(&temp.path().join(IMAGE));
	let catalog = first_leaf(&mut image, &header.forks.catalog, header.geometry);
	let node = Node::decode(&catalog).unwrap();
	for (label, rgb) in [(INHERITED, GOLDENROD), (OVERRIDE, CYAN)] {
		let parent = folder(&node, &header, ROOT_ID, label).id;
		assert_color_record(&mut image, &header, &node, parent, label, rgb);
	}
}

#[test]
fn invalid_colors_fail_preflight_and_preserve_the_destination() {
	const ORIGINAL: &[u8] = b"existing image bytes";
	const INVALID: &[&str] = &[
		"rgb(101%,0,1)",
		"rgb(NaN,0,1)",
		"rgb(0,0,0,.5)",
		"rgb(0,0,1)tail",
		"hsl(180foo,1,.5)",
		"hwb(0,-.1,0)",
		"cmyk(0,0,0,1.1)",
		"gray(inf)",
		"#2468",
		"transparent",
	];
	let temp = tempfile::tempdir().unwrap();
	let output = temp.path().join(IMAGE);
	fs::write(&output, ORIGINAL).unwrap();
	for value in INVALID {
		let project = Project::parse(&format!(
			"{PROJECT_HEADER}\n[build]\noverwrite = true\n[finder]\n{}\n",
			css_background(value)
		))
		.unwrap();
		assert!(
			matches!(
				hfsplus::prepare(
					&project,
					temp.path(),
					options(OutputIntent::Path),
					Default::default(),
				),
				Err(Error::Prepare(planning::Error::Preflight(
					PreflightError::Finder(_)
				)))
			),
			"preflight did not reject {value:?} as an invalid Finder setting",
		);
		assert_eq!(fs::read(&output).unwrap(), ORIGINAL);
	}
}
