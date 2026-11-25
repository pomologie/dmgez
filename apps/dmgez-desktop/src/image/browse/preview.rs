use dmgez_hfsplus::browse;
use dmgez_preview::{
	ItemVisibility,
	actual::{Coverage, EntryKind},
	raster::ActualItem,
};
use dmgez_runtime::preview::actual::render_disk;
use dmgez_vfs::{Disk, LogicalDisk, VolumeId, volume::RawName};
use eframe::egui::{Color32, ColorImage, Context, Rect, TextureHandle, TextureOptions, pos2};

use super::{Error, Location, MAX_DEPTH, Observation, Request};

const TEXTURE_NAME: &str = "dmgez actual folder";
const MAX_TEXTURE_BYTES: usize = 16 << 20;
const RGB_CHANNELS: usize = 3;
const APPLICATION_SUFFIX: [u8; 4] = *b".app";

#[derive(Debug)]
pub(crate) struct Prepared {
	pub location: Location,
	pub visibility: ItemVisibility,
	image: ColorImage,
	coverage: Coverage,
	items: Vec<ActualItem>,
}

pub(crate) struct Presented {
	pub texture: TextureHandle,
	pub coverage: Coverage,
	pub visibility: ItemVisibility,
	pub icons: Vec<Icon>,
}

pub(crate) struct Icon {
	pub bounds: Rect,
	pub name: RawName,
	opens_folder: bool,
}

impl Icon {
	pub(crate) fn opens_folder(&self) -> bool {
		self.opens_folder
	}

	pub(crate) fn can_activate(&self, location: &Location) -> bool {
		!self.opens_folder || location.path.len() < MAX_DEPTH
	}

	pub(crate) fn request(&self, location: &Location) -> Option<Request> {
		if self.opens_folder {
			return location.child(&self.name).map(Request::first);
		}
		Some(Request::Details {
			folder: location.clone(),
			name: self.name.clone(),
		})
	}
}

impl From<ActualItem> for Icon {
	fn from(item: ActualItem) -> Self {
		let bounds = Rect::from_min_max(
			pos2(item.columns.start as f32, item.rows.start as f32),
			pos2(item.columns.end as f32, item.rows.end as f32),
		);
		let application = item
			.name
			.last_chunk::<{ APPLICATION_SUFFIX.len() }>()
			.is_some_and(|suffix| {
				suffix.iter().zip(APPLICATION_SUFFIX).all(|(&unit, byte)| {
					u8::try_from(unit).is_ok_and(|unit| unit.eq_ignore_ascii_case(&byte))
				})
			});
		Self {
			bounds,
			name: RawName::Utf16(item.name),
			opens_folder: item.kind == EntryKind::Directory && !application,
		}
	}
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
			visibility: self.visibility,
			icons: self.items.into_iter().map(Icon::from).collect(),
		})
	}
}

pub(super) fn render(
	disk: &mut Disk<impl LogicalDisk>,
	selected: VolumeId,
	limits: browse::Limits,
	location: Location,
	max_side: usize,
	visibility: ItemVisibility,
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
	let raster = render_disk(disk, selected, limits, &components, visibility)?;
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
		visibility,
		image: ColorImage::from_rgb(size, rgb),
		coverage: raster.coverage(),
		items: raster.into_items(),
	}))
}
