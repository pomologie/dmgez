//! Project design rendering evidence for the D-GUI design-side profile.

use std::{
	path::Path,
	process::{Command, Output},
};

fn invoke(project: &Path, output: &Path, arguments: &[&str]) -> Output {
	Command::new(env!("CARGO_BIN_EXE_dmgez"))
		.args(["preview", "--project"])
		.arg(project)
		.arg("--output")
		.arg(output)
		.args(arguments)
		.output()
		.unwrap()
}

#[test]
fn capabilities_reflect_optional_preview_support() {
	let result = Command::new(env!("CARGO_BIN_EXE_dmgez"))
		.arg("capabilities")
		.output()
		.unwrap();
	assert!(result.status.success());
	let capability = if cfg!(feature = "preview") {
		"compiled"
	} else {
		"not_compiled"
	};
	assert!(
		String::from_utf8_lossy(&result.stdout)
			.lines()
			.any(|line| line == format!("preview = {capability}"))
	);
}

#[cfg(not(feature = "preview"))]
#[test]
fn disabled_preview_fails_before_opening_the_project_or_output() {
	let directory = tempfile::tempdir().unwrap();
	let output = directory.path().join("preview.png");
	let result = invoke(&directory.path().join("absent.toml"), &output, &[]);
	assert!(!result.status.success());
	assert!(String::from_utf8_lossy(&result.stderr).contains("feature not compiled: preview"));
	assert!(!output.exists());
}

#[cfg(feature = "preview")]
mod enabled {
	use std::{fs, io::BufReader};

	use super::*;

	const PROJECT: &str = "schema_version=1\n[image]\nname='Design'\noutput='installer.dmg'\n";
	const OUTPUT_NAME: &str = "preview.png";
	const PROJECT_NAME: &str = "dmg.toml";
	const BACKGROUND: [u8; 3] = [224, 234, 244];
	const ICON_EDGE: [u8; 3] = [103, 132, 129];

	fn assert_success(output: Output) -> String {
		assert!(
			output.status.success(),
			"{:?}",
			String::from_utf8_lossy(&output.stderr)
		);
		String::from_utf8(output.stdout).unwrap()
	}

	fn decode(path: &Path) -> (Vec<u8>, png::OutputInfo) {
		let file = fs::File::open(path).unwrap();
		let mut reader = png::Decoder::new(BufReader::new(file)).read_info().unwrap();
		let mut bytes = vec![0; reader.output_buffer_size().unwrap()];
		let info = reader.next_frame(&mut bytes).unwrap();
		assert_eq!(info.color_type, png::ColorType::Rgb);
		assert_eq!(info.bit_depth, png::BitDepth::Eight);
		(bytes, info)
	}

	fn pixel(bytes: &[u8], row_bytes: usize, x: usize, y: usize) -> [u8; 3] {
		bytes[y * row_bytes + x * 3..][..3].try_into().unwrap()
	}

	#[test]
	fn renders_scrolled_icon_geometry_labels_and_hidden_policy_without_resolving_copies() {
		const CONFIG: &str = r#"
[[contents]]
source='intentionally-absent.txt'
destination='/Guide.txt'
hide_extension=true
position=[40, 25]
[[contents]]
type='directory'
destination='/Private'
hidden=true
position=[105, 74]
[finder]
window_size=[160, 100]
background={kind='color', value='#e0eaf4'}
[finder.icon_view]
icon_size=20
text_size=14
scroll_position=[10, 4]
label_position='right'
"#;
		let directory = tempfile::tempdir().unwrap();
		let root = directory.path();
		let project = root.join(PROJECT_NAME);
		let output = root.join(OUTPUT_NAME);
		let source = format!("{PROJECT}{CONFIG}");
		fs::write(&project, &source).unwrap();
		assert_eq!(
			assert_success(invoke(&project, &output, &[])),
			"mode=design\nwidth=160\nheight=100\n"
		);
		let (bytes, info) = decode(&output);
		assert_eq!((info.width, info.height), (160, 100));
		assert_eq!(pixel(&bytes, info.line_size, 20, 11), ICON_EDGE);
		assert_eq!(pixel(&bytes, info.line_size, 19, 11), BACKGROUND);
		assert_eq!(pixel(&bytes, info.line_size, 85, 60), BACKGROUND);
		let hidden = root.join("with-hidden.png");
		assert_success(invoke(&project, &hidden, &["--show-hidden"]));
		let (shown, info) = decode(&hidden);
		assert_eq!(pixel(&shown, info.line_size, 85, 60), ICON_EDGE);
		let ink_pixels = |bytes: &[u8]| {
			(8..35)
				.flat_map(|y| (45..100).map(move |x| (x, y)))
				.filter(|(x, y)| pixel(bytes, info.line_size, *x, *y)[0] < 160)
				.count()
		};
		assert!(
			ink_pixels(&bytes) > 20,
			"the label must contain drawn glyphs"
		);
		let alternate = root.join("extension.toml");
		let with_extension = root.join("extension.png");
		fs::write(
			&alternate,
			source.replace("hide_extension=true", "hide_extension=false"),
		)
		.unwrap();
		assert_success(invoke(&alternate, &with_extension, &[]));
		let (extended, _) = decode(&with_extension);
		assert!(ink_pixels(&extended) > ink_pixels(&bytes));
		assert_eq!(fs::read_to_string(project).unwrap(), source);
		assert!(!root.join("installer.dmg").exists());
		assert!(!root.join("intentionally-absent.txt").exists());
	}

	#[test]
	fn paints_a_project_relative_png_background_in_the_selected_folder() {
		const CONFIG: &str = r#"
[[contents]]
type='directory'
destination='/Docs'
[[finder.folders]]
path='/Docs'
window_size=[80, 64]
background={kind='image', source='background.png', retina='unused-2x.png'}
"#;
		let directory = tempfile::tempdir().unwrap();
		let manifest_dir = directory.path().join("project");
		fs::create_dir(&manifest_dir).unwrap();
		let input = manifest_dir.join("background.png");
		let mut encoder = png::Encoder::new(fs::File::create(&input).unwrap(), 2, 1);
		encoder.set_color(png::ColorType::Rgba);
		encoder.set_depth(png::BitDepth::Eight);
		let mut writer = encoder.write_header().unwrap();
		writer
			.write_image_data(&[255, 0, 0, 255, 0, 0, 255, 128])
			.unwrap();
		writer.finish().unwrap();
		let original = fs::read(&input).unwrap();
		let project = manifest_dir.join(PROJECT_NAME);
		fs::write(&project, format!("{PROJECT}{CONFIG}")).unwrap();
		let output = directory.path().join(OUTPUT_NAME);
		assert_eq!(
			assert_success(invoke(&project, &output, &["--folder", "/Docs"])),
			"mode=design\nwidth=80\nheight=64\n"
		);
		let (bytes, info) = decode(&output);
		assert_eq!(pixel(&bytes, info.line_size, 0, 0), [255, 0, 0]);
		assert_eq!(pixel(&bytes, info.line_size, 1, 0), [122, 124, 252]);
		assert_eq!(pixel(&bytes, info.line_size, 2, 0), [245, 249, 248]);
		assert_eq!(fs::read(input).unwrap(), original);
	}

	#[test]
	fn refuses_unsupported_layouts_and_assets_without_creating_or_clobbering_output() {
		let directory = tempfile::tempdir().unwrap();
		let project = directory.path().join(PROJECT_NAME);
		let output = directory.path().join(OUTPUT_NAME);
		let oversized = fs::File::create(directory.path().join("oversized.png")).unwrap();
		oversized.set_len((16 << 20) + 1).unwrap();
		fs::write(directory.path().join("malformed.png"), b"invalid PNG").unwrap();
		for (config, diagnostic) in [
			("[finder]\nwindow_size=[4096, 4096]\n", "dimensions exceed"),
			("[finder]\ndefault_view='list-view'\n", "unsupported view"),
			(
				"[finder.icon_view]\narrange_by='name'\n",
				"manually arranged",
			),
			(
				"[finder]\nbackground={kind='template', name='builtin-arrow'}\n",
				"background is outside",
			),
			(
				"[finder]\nbackground={kind='image', source='absent.png'}\n",
				"background source could not be opened",
			),
			(
				"[finder]\nbackground={kind='image', source='oversized.png'}\n",
				"input byte budget",
			),
			(
				"[finder]\nbackground={kind='image', source='malformed.png'}\n",
				"could not be decoded",
			),
		] {
			fs::write(&project, format!("{PROJECT}{config}")).unwrap();
			let result = invoke(&project, &output, &[]);
			assert!(!result.status.success(), "{config}");
			assert!(
				String::from_utf8_lossy(&result.stderr).contains(diagnostic),
				"{:?}",
				result.stderr
			);
			assert!(!output.exists());
		}
		fs::write(&project, PROJECT).unwrap();
		let missing_folder = invoke(&project, &output, &["--folder", "/Absent"]);
		assert!(!missing_folder.status.success() && !output.exists());
		fs::write(&output, b"keep me").unwrap();
		let result = invoke(&project, &output, &[]);
		assert!(!result.status.success());
		assert_eq!(fs::read(&output).unwrap(), b"keep me");
		let alias = invoke(&project, &project, &[]);
		assert!(!alias.status.success());
		assert_eq!(fs::read_to_string(&project).unwrap(), PROJECT);
	}
}
