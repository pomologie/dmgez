use std::error::Error as StdError;

use dmgez_vfs::volume::{
	FINDER_INFO_BYTES, FinderInfo, NodeHandle, ViewError, VolumeBackend, VolumeView,
};

use super::{Error, payloads::Payloads};

/// Stopped per-object FinderInfo comparison on one side of a common path.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum FinderInfoError<E: StdError + 'static> {
	#[error("effective FinderInfo is unavailable")]
	Unavailable,
	#[error("could not read effective FinderInfo")]
	Read(#[source] ViewError<E>),
}

pub(super) fn equivalent<L: VolumeBackend, R: VolumeBackend>(
	left: (&mut VolumeView<L>, &NodeHandle<L::Id>),
	right: (&mut VolumeView<R>, &NodeHandle<R::Id>),
	path: &[String],
	payloads: &mut Payloads<impl FnMut() -> bool>,
) -> Result<bool, Error<L::Error, R::Error>> {
	payloads.poll()?;
	let left = read(left.0, left.1).map_err(|source| Error::LeftFinderInfo {
		path: path.to_vec(),
		source,
	})?;
	payloads.poll()?;
	let right = read(right.0, right.1).map_err(|source| Error::RightFinderInfo {
		path: path.to_vec(),
		source,
	})?;
	if left.is_some() || right.is_some() {
		// Establish presence through bounded metadata reads. Charge the larger
		// of the two logical lengths, as for other paired whole values.
		payloads.charge_bytes(FINDER_INFO_BYTES as u64)?;
	}
	Ok(left == right)
}

fn read<B: VolumeBackend>(
	view: &mut VolumeView<B>,
	node: &NodeHandle<B::Id>,
) -> Result<Option<[u8; FINDER_INFO_BYTES]>, FinderInfoError<B::Error>> {
	match view.finder_info(node).map_err(FinderInfoError::Read)? {
		FinderInfo::Unavailable => Err(FinderInfoError::Unavailable),
		FinderInfo::Absent => Ok(None),
		FinderInfo::Present(bytes) => Ok(Some(bytes)),
	}
}
