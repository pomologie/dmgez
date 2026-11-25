use std::{io, path::Path};

use dmgez_config::VirtualPath;
use dmgez_hfsplus_read::{browse::Limits as HfsLimits, preview::capture_disk};
use dmgez_udif::reader::ReadAt as _;
use dmgez_vfs::{Disk, Limits as DiskLimits};

use crate::{
	CliResult,
	inspect::open_source,
	volume_input::{select, with_logical_disk},
};

use super::{Arguments, publish};

const ACTUAL_MODE: &str = "actual";
const STORE_KEY: &str = "finder_store";
const LAYOUT_KEY: &str = "finder_layout";
const BACKGROUND_KEY: &str = "background";
const OBSERVED_KEY: &str = "observed_positions";
const FALLBACK_KEY: &str = "fallback_positions";
const MAX_FS_READ_REQUESTS: u64 = 131_072;
const MAX_FS_READ_BYTES: u64 = 256 << 20;

pub(super) fn image(path: &Path, arguments: &Arguments) -> CliResult {
	let folder: VirtualPath = arguments.folder.parse()?;
	let components: Vec<_> = folder
		.components()
		.map(|name| name.encode_utf16().collect())
		.collect();
	let source = open_source(path)?;
	let snapshot = with_logical_disk(&source, arguments.raw, |logical| {
		let mut disk = Disk::discover(logical, DiskLimits::default())?;
		let selected = select(&disk, arguments.partition)?;
		let limits = HfsLimits {
			max_read_requests: MAX_FS_READ_REQUESTS,
			max_read_bytes: MAX_FS_READ_BYTES,
			..HfsLimits::default()
		};
		Ok(capture_disk(&mut disk, selected, limits, &components)?)
	})?;
	let preview = snapshot.render(arguments.visibility())?;
	source.check_unchanged()?;
	publish(
		&arguments.output,
		ACTUAL_MODE,
		preview.size(),
		|output| preview.write_png(output).map_err(io::Error::other),
		|| source.check_unchanged().map_err(io::Error::other),
	)?;
	let coverage = preview.coverage();
	println!("{STORE_KEY}={}", coverage.store.as_str());
	println!("{LAYOUT_KEY}={}", coverage.layout.as_str());
	println!("{BACKGROUND_KEY}={}", coverage.background.as_str());
	println!("{OBSERVED_KEY}={}", coverage.observed_positions);
	println!("{FALLBACK_KEY}={}", coverage.fallback_positions);
	Ok(())
}
