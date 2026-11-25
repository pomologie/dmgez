//! Bounded inert previews of an admitted regular file's data fork.

use dmgez_vfs::volume::{ForkKind, NodeHandle, NodeKind, ViewError, VolumeBackend, VolumeView};

const MAX_TEXT_BYTES: usize = 64 << 10;

pub(super) enum Request {
	MetadataOnly,
	Text,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Preview {
	Text { value: String, extent: Extent },
	Binary,
	UnsupportedEncoding,
	UnsupportedKind,
	AbsentDataFork,
	AllocationUnavailable,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Extent {
	Complete,
	Prefix { file_bytes: u64 },
}

pub(super) fn read<B: VolumeBackend>(
	view: &mut VolumeView<B>,
	node: &NodeHandle<B::Id>,
	kind: NodeKind,
) -> Result<Preview, ViewError<B::Error>> {
	if kind != NodeKind::RegularFile {
		return Ok(Preview::UnsupportedKind);
	}
	Ok(view
		.with_fork(
			node,
			ForkKind::Data,
			|fork| -> Result<_, ViewError<B::Error>> {
				let length = fork.len().min(MAX_TEXT_BYTES as u64) as usize;
				let mut bytes = Vec::new();
				if bytes.try_reserve_exact(length).is_err() {
					return Ok(Preview::AllocationUnavailable);
				}
				bytes.resize(length, 0);
				fork.read_exact_at(0, &mut bytes)?;
				Ok(classify(bytes, fork.len()))
			},
		)?
		.transpose()?
		.unwrap_or(Preview::AbsentDataFork))
}

fn classify(bytes: Vec<u8>, file_bytes: u64) -> Preview {
	if bytes
		.iter()
		.copied()
		.any(|byte| byte.is_ascii_control() && !matches!(byte, b'\t' | b'\n' | b'\r'))
	{
		return Preview::Binary;
	}
	let extent = if bytes.len() as u64 == file_bytes {
		Extent::Complete
	} else {
		Extent::Prefix { file_bytes }
	};
	let value = match String::from_utf8(bytes) {
		Ok(value) => value,
		Err(error)
			if matches!(extent, Extent::Prefix { .. })
				&& error.utf8_error().error_len().is_none() =>
		{
			// A sample can end inside one UTF-8 scalar. Preserve only its checked prefix.
			let end = error.utf8_error().valid_up_to();
			let mut prefix = error.into_bytes();
			prefix.truncate(end);
			let Ok(value) = String::from_utf8(prefix) else {
				return Preview::UnsupportedEncoding;
			};
			value
		}
		Err(_) => return Preview::UnsupportedEncoding,
	};
	if value
		.chars()
		.any(|character| character.is_control() && !matches!(character, '\t' | '\n' | '\r'))
	{
		return Preview::Binary;
	}
	Preview::Text { value, extent }
}

#[cfg(test)]
mod tests;
