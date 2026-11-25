use std::io;

use dmgez_inspector::volume_queries::{
	self, DEFAULT_LIST_LIMIT, DEFAULT_TREE_DEPTH, DEFAULT_TREE_LIMIT,
};
use dmgez_vfs::volume::{ForkKind, VolumeBackend, VolumeView};

use crate::CliResult;

#[cfg(feature = "apfs-read")]
use super::apfs::{VolumeIdentity, checkpoint, volume_selector, with_selector_guidance};
use super::{
	Fork, InputArguments, Query,
	report::{ReadVolume, Report},
};

#[cfg(feature = "hfsplus-read")]
pub(super) fn hfsplus(query: &Query<'_>) -> CliResult {
	let input = query.input();
	let report =
		crate::volume_input::with_hfs_view(&input.image, input.raw, input.partition, |view| {
			query.read(view)
		})?;
	report.write(query.json(), ReadVolume::Hfsplus)
}

#[cfg(feature = "apfs-read")]
pub(super) fn apfs(query: &Query<'_>) -> CliResult {
	let input = query.input();
	let selection = query.selection();
	let selector = selection
		.volume
		.as_deref()
		.map(volume_selector)
		.transpose()?;
	let (report, identity) = crate::volume_input::with_apfs_view(
		&input.image,
		input.raw,
		input.partition,
		selector,
		checkpoint(selection.xid),
		|view, info| Ok((query.read(view)?, VolumeIdentity::from(info))),
	)
	.map_err(with_selector_guidance)?;
	report.write(query.json(), ReadVolume::Apfs(identity))
}

impl Query<'_> {
	fn input(&self) -> &InputArguments {
		match self {
			Self::List(args) => &args.input,
			Self::Tree(args) => &args.input,
			Self::Stat(args) => &args.input,
			Self::Cat(args) => &args.input,
		}
	}

	fn json(&self) -> bool {
		match self {
			Self::List(args) => args.json,
			Self::Tree(args) => args.json,
			Self::Stat(args) => args.json,
			Self::Cat(_) => false,
		}
	}

	fn read<B: VolumeBackend>(&self, view: &mut VolumeView<B>) -> CliResult<Report> {
		match self {
			Self::List(args) => Ok(Report::List(volume_queries::list(
				view,
				&args.path,
				args.offset,
				args.limit.unwrap_or(DEFAULT_LIST_LIMIT),
			)?)),
			Self::Tree(args) => Ok(Report::Tree(volume_queries::tree(
				view,
				&args.path,
				args.depth.unwrap_or(DEFAULT_TREE_DEPTH),
				args.limit.unwrap_or(DEFAULT_TREE_LIMIT),
			)?)),
			Self::Stat(args) => Ok(Report::Stat(volume_queries::stat(view, &args.path)?)),
			Self::Cat(args) => {
				let fork = match args.fork {
					Fork::Data => ForkKind::Data,
					Fork::Resource => ForkKind::Resource,
				};
				let mut output = io::stdout().lock();
				volume_queries::cat(view, &args.path, fork, &mut output)?;
				Ok(Report::Cat(output))
			}
		}
	}
}
