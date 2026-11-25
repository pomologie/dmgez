use dmgez_hfsplus_read::browse;
use dmgez_preview::{ItemVisibility, actual::Coverage};
use dmgez_runtime::preview::actual::render_disk;
use dmgez_vfs::{Disk, LogicalDisk, VolumeId, volume::RawName};
use eframe::egui::{Color32, ColorImage, Context, TextureHandle, TextureOptions};

use super::{Error, Location, MAX_DEPTH, Observation};

const TEXTURE_NAME: &str = "dmgez actual folder";
const MAX_TEXTURE_BYTES: usize = 16 << 20;
const RGB_CHANNELS: usize = 3;

#[derive(Debug)]
pub(crate) struct Prepared {
	pub location: Location,
	image: ColorImage,
	coverage: Coverage,
}

pub(crate) struct Presented {
	pub texture: TextureHandle,
	pub coverage: Coverage,
}

impl Prepared {
	pub(crate) fn present(self, context: &Context) -> Result<Presented, Error> {
		let max_side = context.input(|input| input.max_texture_side);
		if self.image.size.iter().any(|&side| side > max_side) {
			return Err(Error::PreviewTextureLimit);
		}
		let texture = context.load_texture(TEXTURE_NAME, self.image, TextureOptions::LINEAR);
		Ok(Presented {
			texture,
			coverage: self.coverage,
		})
	}
}

pub(super) fn render(
	disk: &mut Disk<impl LogicalDisk>,
	selected: VolumeId,
	limits: browse::Limits,
	location: Location,
	max_side: usize,
) -> Result<Observation, Error> {
	if location.path.len() > MAX_DEPTH {
		return Err(Error::DepthLimit);
	}
	let components = location
		.path
		.iter()
		.map(|name| match name {
			RawName::Utf16(units) => Ok(units.clone()),
			_ => Err(Error::PreviewNameEncoding),
		})
		.collect::<Result<Vec<_>, _>>()?;
	let raster = render_disk(disk, selected, limits, &components, ItemVisibility::Visible)?;
	let (width, height) = raster.size();
	let size = [width as usize, height as usize];
	let pixels = size[0]
		.checked_mul(size[1])
		.filter(|&pixels| pixels <= MAX_TEXTURE_BYTES / size_of::<Color32>())
		.ok_or(Error::PreviewTextureLimit)?;
	if size.contains(&0) || size.iter().any(|&side| side > max_side) {
		return Err(Error::PreviewTextureLimit);
	}
	let rgb = raster.rgb();
	if rgb.len() != pixels * RGB_CHANNELS {
		return Err(Error::PreviewTextureData);
	}
	Ok(Observation::Preview(Prepared {
		location,
		image: ColorImage::from_rgb(size, rgb),
		coverage: raster.coverage(),
	}))
}
