use std::{error::Error as StdError, fmt};

use dmgez_preview::png_image::{self, FILE_INPUT_BYTES, SIGNATURE};
use dmgez_udif::Cancellation;
use dmgez_vfs::volume::{
	Fork, ForkKind, NodeHandle, NodeKind, ViewError, VolumeBackend, VolumeView,
};
use eframe::egui::{Color32, ColorImage, Context, TextureHandle, TextureOptions};
use thiserror::Error;

use super::super::Details;

const TEXTURE_NAME: &str = "dmgez selected PNG";

#[derive(Debug, Error)]
pub(crate) enum Issue {
	#[error("PNG preview is available for regular files")]
	UnsupportedKind,
	#[error("This file has no data fork")]
	AbsentDataFork,
	#[error("This data fork has no PNG signature")]
	UnsupportedFormat,
	#[error("This PNG exceeds the {FILE_INPUT_BYTES}-byte input limit")]
	InputBytes,
	#[error("Memory for the PNG preview is unavailable")]
	Allocation,
	#[error("This PNG exceeds the renderer's texture limit")]
	TextureLimit,
	#[error(transparent)]
	Decode(#[from] png_image::Error),
}

pub(crate) type Preview = Result<Presented, Issue>;

pub(crate) struct Presented {
	pub texture: TextureHandle,
	pub size: [usize; 2],
}

impl fmt::Debug for Presented {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("Presented")
			.field("size", &self.size)
			.finish_non_exhaustive()
	}
}

#[derive(Debug)]
pub(crate) struct Prepared {
	pub details: Details,
	pub image: Result<ColorImage, Issue>,
}

impl Prepared {
	pub(crate) fn present(self, context: &Context) -> Details {
		let mut details = self.details;
		let result = self.image.and_then(|image| {
			let max_side = context.input(|input| input.max_texture_side);
			let size = image.size;
			if size.iter().any(|&side| side > max_side) {
				return Err(Issue::TextureLimit);
			}
			let texture = context.load_texture(TEXTURE_NAME, image, TextureOptions::LINEAR);
			Ok(Presented { texture, size })
		});
		details.png = Some(Box::new(result));
		details
	}
}

pub(in crate::image::browse) fn read<B: VolumeBackend>(
	view: &mut VolumeView<B>,
	node: &NodeHandle<B::Id>,
	kind: NodeKind,
	cancellation: &Cancellation,
) -> Result<Result<ColorImage, Issue>, ViewError<B::Error>> {
	if kind != NodeKind::RegularFile {
		return Ok(Err(Issue::UnsupportedKind));
	}
	Ok(view
		.with_fork(node, ForkKind::Data, |fork| read_fork(fork, cancellation))?
		.transpose()?
		.unwrap_or(Err(Issue::AbsentDataFork)))
}

fn read_fork<E: StdError + 'static>(
	fork: &mut Fork<'_, E>,
	cancellation: &Cancellation,
) -> Result<Result<ColorImage, Issue>, ViewError<E>> {
	let mut signature = [0; SIGNATURE.len()];
	let signature_bytes = signature.len() as u64;
	if fork.len() < signature_bytes {
		return Ok(Err(Issue::UnsupportedFormat));
	}
	fork.read_exact_at(0, &mut signature)?;
	if signature != SIGNATURE {
		return Ok(Err(Issue::UnsupportedFormat));
	}
	if fork.len() > FILE_INPUT_BYTES as u64 {
		return Ok(Err(Issue::InputBytes));
	}
	let mut encoded = Vec::new();
	let length = fork.len() as usize;
	if encoded.try_reserve_exact(length).is_err() {
		return Ok(Err(Issue::Allocation));
	}
	encoded.extend_from_slice(&signature);
	encoded.resize(length, 0);
	fork.read_exact_at(signature_bytes, &mut encoded[SIGNATURE.len()..])?;
	Ok(decode(&encoded, cancellation))
}

fn decode(encoded: &[u8], cancellation: &Cancellation) -> Result<ColorImage, Issue> {
	let image = png_image::decode_file(encoded, || cancellation.is_cancelled())?;
	let size = image.size();
	let rgba = image.rgba_pixels();
	let mut pixels = Vec::new();
	pixels
		.try_reserve_exact(rgba.len())
		.map_err(|_| Issue::Allocation)?;
	for (index, [red, green, blue, alpha]) in rgba.enumerate() {
		if index.is_multiple_of(size[0]) && cancellation.is_cancelled() {
			return Err(png_image::Error::Cancelled.into());
		}
		pixels.push(Color32::from_rgba_unmultiplied(red, green, blue, alpha));
	}
	Ok(ColorImage::new(size, pixels))
}
