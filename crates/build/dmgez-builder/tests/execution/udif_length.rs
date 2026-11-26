use dmgez_config::ImageFormat;

use super::*;

const AUTO_SIZE_PROJECT: &str =
	"schema_version = 1\n[image]\nname = 'Test'\noutput = 'built.dmg'\nformat = 'udro'\n";

#[test]
fn refuses_to_publish_an_udif_stage_when_the_stream_length_disagrees() {
	let root = tempfile::tempdir().unwrap();
	let path = root.path().join(DESTINATION);
	let mut project = Project::parse(AUTO_SIZE_PROJECT).unwrap();
	let logical_bytes = 2 * LOGICAL_BYTES;
	let source = [b'A'; 3 * LOGICAL_BYTES];
	let formats = [
		ImageFormat::Udro,
		#[cfg(feature = "zlib")]
		ImageFormat::Udzo,
	];
	for format in formats {
		project.image.format = format;
		let manifest = discover(&project, root.path(), Limits::default()).unwrap();
		let profile = profile(&project, OutputIntent::Path);
		for length in [LOGICAL_BYTES, source.len()] {
			let plan = BuildPlan::new(&profile, &manifest, &source[..length], logical_bytes as u64)
				.unwrap();
			assert!(matches!(
				plan.publish(&path, None),
				Err(ExecuteError::Path(PublishError::Write(ImageError::Writer(
					dmgez_udif::Error::SourceLength(_)
				))))
			));
			assert!(!path.exists());
			assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
		}
	}
}
