//! Planner evidence for captured PNG badge sources and generated icon budgets.

#![cfg(all(feature = "badge-icons", any(unix, windows)))]

use std::{fs, num::NonZeroUsize, path::Path};

use dmgez_builder::{
	planning::{self, PreparedBuild},
	preflight::{Availability, BuildCapabilities, OutputIntent, PreflightOptions, ReleaseIntent},
	presentation,
};
use dmgez_config::Project;
use dmgez_finder::volume_icon;
use dmgez_udif::writer::Cancellation;

const BADGE_SOURCE: &str = "badge.png";
const OUTPUT: &str = "output.dmg";
const BADGE_WIDTH: u32 = 17;
const BADGE_HEIGHT: u32 = 13;
const BADGE_RGBA: [u8; 4] = [118, 30, 160, 190];
const FINDER_INFO_BYTES: usize = 32;
const ROOT_CUSTOM_ICON_FLAGS: [u8; 2] = 0x0400_u16.to_be_bytes();
const ROOT_FINDER_FLAGS: std::ops::Range<usize> = 8..10;

fn write_badge(folder: &Path) {
	let output = fs::File::create(folder.join(BADGE_SOURCE)).unwrap();
	let mut encoder = png::Encoder::new(output, BADGE_WIDTH, BADGE_HEIGHT);
	encoder.set_color(png::ColorType::Rgba);
	let mut writer = encoder.write_header().unwrap();
	writer
		.write_image_data(&BADGE_RGBA.repeat((BADGE_WIDTH * BADGE_HEIGHT) as usize))
		.unwrap();
	writer.finish().unwrap();
}

fn prepare(base: &Path, settings: &str) -> Result<PreparedBuild, planning::Error> {
	let project = Project::parse(&format!(
		"schema_version = 1\n[image]\nname = 'Badged image'\noutput = '{OUTPUT}'\nformat = 'udro'\n\
		[volume_icon]\nbadge = '{BADGE_SOURCE}'\n{settings}"
	))
	.unwrap();
	let mut capabilities = BuildCapabilities::default();
	capabilities.hfsplus_ascii = Availability::Available;
	capabilities.finder_volume_icon = Availability::Available;
	let options = PreflightOptions {
		capabilities,
		output: OutputIntent::Path,
		release: ReleaseIntent::Unsigned,
		host_threads: NonZeroUsize::MIN,
	};
	planning::prepare(&project, base, options, Cancellation::default())
}

#[test]
fn badge_reserves_one_deterministic_volume_icon_and_the_root_flag() {
	let directory = tempfile::tempdir().unwrap();
	write_badge(directory.path());
	let first = prepare(directory.path(), "").unwrap();
	let second = prepare(directory.path(), "").unwrap();
	let [icon] = first.finder_assets() else {
		panic!("the badge should reserve one volume icon");
	};
	assert_eq!(icon.path.as_str(), format!("/{}", volume_icon::NAME));
	assert_eq!(first.finder_assets(), second.finder_assets());
	volume_icon::validate(&icon.bytes).unwrap();
	let mut finder_info = [0; FINDER_INFO_BYTES];
	first.apply_root_finder_info(&mut finder_info);
	assert_eq!(finder_info[ROOT_FINDER_FLAGS], ROOT_CUSTOM_ICON_FLAGS);
	assert!(!directory.path().join(OUTPUT).exists());
}

#[test]
fn badge_planning_accounts_for_working_memory_and_emitted_metadata() {
	let directory = tempfile::tempdir().unwrap();
	write_badge(directory.path());
	let plan = prepare(directory.path(), "").unwrap();
	let asset_bytes = plan.finder_assets()[0].bytes.len() as u64;
	let too_little_metadata = plan.manifest().metadata_bytes() + asset_bytes / 2;
	let metadata_settings =
		format!("[performance]\nmax_metadata_bytes = '{too_little_metadata}B'\n");
	assert!(matches!(
		prepare(directory.path(), &metadata_settings),
		Err(planning::Error::Presentation(
			presentation::Error::MetadataBudget
		))
	));
	assert!(matches!(
		prepare(directory.path(), "[performance]\nmemory_limit = '4MiB'\n"),
		Err(planning::Error::Presentation(
			presentation::Error::VolumeIconMemory
		))
	));
}
