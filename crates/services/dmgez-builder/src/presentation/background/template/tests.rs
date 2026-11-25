use std::{io::Cursor, num::NonZeroUsize};

use dmgez_config::Project;
use tiff::{
	ColorType,
	decoder::{Decoder, DecodingResult},
	tags::{CompressionMethod, ResolutionUnit, Tag},
};

use crate::preflight::{
	Availability, BuildCapabilities, OutputIntent, PreflightOptions, ReleaseIntent, preflight,
};

use super::super::TIFF_TOP_LEFT;
use super::{
	ARROW_RGB, BACKGROUND_RGB, BASE_DPI, Error, HEIGHT, RETINA_DPI, RETINA_SCALE, ROWS_PER_STRIP,
	STANDARD_SCALE, WIDTH, render,
};

const OUTPUT_BUDGET: usize = 64 << 10;

fn profile(memory_limit: &str) -> crate::preflight::ExecutionProfile {
	let project = Project::parse(&format!(
		"schema_version = 1\n[image]\nname = 'Arrow'\noutput = 'out.dmg'\nformat = 'udro'\n\
		 [performance]\nmemory_limit = '{memory_limit}'\n"
	))
	.unwrap();
	let capabilities = BuildCapabilities {
		hfsplus_ascii: Availability::Available,
		..BuildCapabilities::default()
	};
	preflight(
		&project,
		PreflightOptions {
			capabilities,
			output: OutputIntent::Path,
			release: ReleaseIntent::Unsigned,
			host_threads: NonZeroUsize::MIN,
		},
	)
	.unwrap()
}

#[test]
fn renders_deterministic_standard_and_retina_arrows() {
	let profile = profile("8MiB");
	let bytes = render(&profile, OUTPUT_BUDGET).unwrap();
	assert_eq!(render(&profile, OUTPUT_BUDGET).unwrap(), bytes);

	let mut tiff = Decoder::new(Cursor::new(bytes)).unwrap();
	for (scale, dpi) in [(STANDARD_SCALE, BASE_DPI), (RETINA_SCALE, RETINA_DPI)] {
		let (width, height) = tiff.dimensions().unwrap();
		assert_eq!((width, height), (WIDTH * scale, HEIGHT * scale));
		assert_eq!(tiff.colortype().unwrap(), ColorType::RGB(8));
		assert_eq!(
			tiff.get_tag_u32(Tag::Compression).unwrap(),
			u32::from(CompressionMethod::Deflate.to_u16())
		);
		assert_eq!(
			tiff.get_tag_u32(Tag::ResolutionUnit).unwrap(),
			u32::from(ResolutionUnit::Inch.to_u16())
		);
		assert_eq!(
			tiff.get_tag_u32(Tag::Orientation).unwrap(),
			u32::from(TIFF_TOP_LEFT)
		);
		assert_eq!(tiff.get_tag_u32(Tag::RowsPerStrip).unwrap(), ROWS_PER_STRIP);
		for resolution in [Tag::XResolution, Tag::YResolution] {
			assert_eq!(tiff.get_tag_u32_vec(resolution).unwrap(), [dpi, 1]);
		}
		let DecodingResult::U8(raster) = tiff.read_image().unwrap() else {
			panic!("expected eight-bit RGB");
		};
		let at = |x, y| {
			let start = ((y * scale * width + x * scale) as usize) * BACKGROUND_RGB.len();
			&raster[start..start + BACKGROUND_RGB.len()]
		};
		assert_eq!(at(20, 20), BACKGROUND_RGB);
		assert_eq!(at(320, 140), ARROW_RGB);
		assert_eq!(at(337, 115), ARROW_RGB);
		assert_eq!(at(396, 120), BACKGROUND_RGB);
		assert_eq!(at(396, 140), ARROW_RGB);
		assert!(
			raster
				.as_chunks::<{ BACKGROUND_RGB.len() }>()
				.0
				.iter()
				.any(|&pixel| pixel != BACKGROUND_RGB && pixel != ARROW_RGB)
		);
		if scale == STANDARD_SCALE {
			tiff.next_image().unwrap();
		}
	}
	assert!(!tiff.more_images());
}

#[test]
fn enforces_both_memory_budgets_and_observes_cancellation() {
	let room = profile("8MiB");
	let bytes = render(&room, OUTPUT_BUDGET).unwrap();
	assert!(matches!(render(&room, 0), Err(Error::MetadataBudget)));
	assert!(matches!(
		render(&room, bytes.len() - 1),
		Err(Error::MetadataBudget)
	));
	assert!(matches!(
		render(&profile("3MiB"), OUTPUT_BUDGET),
		Err(Error::BackgroundMemory)
	));
	room.write_options().cancellation.cancel();
	assert!(matches!(
		render(&room, OUTPUT_BUDGET),
		Err(Error::Cancelled)
	));
}
