use std::{fs::File, io, path::Path};

use dmgez_core::process;

use super::{
	Attachment, DetachOutcome, Error, Observation, Request,
	record::{CanonicalPath, Device, MountedIdentity},
};

mod files;
mod protocol;
#[cfg(test)]
mod tests;

use files::{HeldDirectory, HeldImage};
use protocol::{Inventory, Relation};

pub(super) fn attach(request: &Request) -> Result<Attachment, Error> {
	let image = admit_path(request.image())?;
	let mountpoint = admit_path(request.mountpoint())?;
	let source = HeldImage::open(image.as_path())?;
	let underlying = HeldDirectory::open(mountpoint.as_path())?;
	underlying.verify_empty(mountpoint.as_path())?;
	source.admit_format(image.as_path())?;
	let before = protocol::inventory()?;
	let pending = Pending {
		image,
		mountpoint,
		source,
		underlying,
		before,
	};
	pending.attach()
}

struct Pending {
	image: CanonicalPath,
	mountpoint: CanonicalPath,
	source: HeldImage,
	underlying: HeldDirectory,
	before: Inventory,
}

impl Pending {
	fn attach(self) -> Result<Attachment, Error> {
		let image = self.image.as_path();
		let mountpoint = self.mountpoint.as_path();
		if self
			.before
			.contains_selection(image, self.source.stamp.id, mountpoint)
		{
			return Err(Error::AlreadyAttached);
		}
		self.source.verify(image)?;
		self.underlying.verify_empty(mountpoint)?;

		let output = match protocol::attach(image, mountpoint) {
			Ok(output) => output,
			Err(error @ process::Error::Spawn(_)) => return Err(error.into()),
			Err(error) => return Err(attach_uncertain(error.into())),
		};
		if !output.status.success() {
			let declined = Error::AttachRefused {
				status: output.status,
			};
			return match self.confirm_refusal() {
				Ok(()) => Err(declined),
				Err(error) => Err(attach_uncertain(error)),
			};
		}
		self.confirm_attachment(&output.stdout)
			.map_err(attach_uncertain)
	}

	fn confirm_refusal(&self) -> Result<(), Error> {
		let image = self.image.as_path();
		let mountpoint = self.mountpoint.as_path();
		let current = protocol::inventory()?;
		if current.contains_selection(image, self.source.stamp.id, mountpoint) {
			return Err(Error::AlreadyAttached);
		}
		self.underlying.verify_empty(mountpoint)?;
		self.source.verify(image)
	}

	fn confirm_attachment(self, xml: &[u8]) -> Result<Attachment, Error> {
		let image = self.image.as_path();
		let mountpoint = self.mountpoint.as_path();
		let device = protocol::attached_device(xml, mountpoint)?;
		if self.before.contains_device(&device) {
			return Err(Error::AlreadyAttached);
		}
		let current = protocol::inventory()?;
		if current.relation(image, self.source.stamp.id, mountpoint, &device) != Relation::Exact {
			return Err(Error::InvalidEvidence);
		}
		let node = files::device_stamp(&device)?.ok_or(Error::IdentityMismatch)?;
		let mount = mounted_identity(mountpoint, &device)?.ok_or(Error::IdentityMismatch)?;
		self.source.verify(image)?;
		Ok(Attachment {
			image: self.image,
			mountpoint: self.mountpoint,
			device,
			source: self.source.stamp,
			underlying: self.underlying.id,
			mount,
			node,
		})
	}
}

pub(super) fn observe(attachment: &Attachment) -> Result<Observation, Error> {
	let image = attachment.image();
	let mountpoint = attachment.mountpoint();
	let current = protocol::inventory()?;
	let relation = current.relation(image, attachment.source.id, mountpoint, &attachment.device);
	let node = files::device_stamp(&attachment.device)?;
	match relation {
		Relation::Exact
			if node == Some(attachment.node)
				&& mounted_identity(mountpoint, &attachment.device)? == Some(attachment.mount)
				&& files::named_image_matches(image, attachment.source)? =>
		{
			Ok(Observation::Mounted)
		}
		Relation::Absent if node.is_none() => {
			let directory = HeldDirectory::open(mountpoint)?;
			if !directory.is_mountpoint(mountpoint)? && directory.id == attachment.underlying {
				directory.verify_name(mountpoint)?;
				return Ok(Observation::Detached);
			}
			Ok(Observation::IdentityMismatch)
		}
		_ => Ok(Observation::IdentityMismatch),
	}
}

pub(super) fn open_attachment_source(attachment: &Attachment) -> Result<File, Error> {
	HeldImage::open_matching(attachment.image(), attachment.source)
}

pub(super) fn check_attachment_source(attachment: &Attachment, file: &File) -> Result<(), Error> {
	files::exact_path(attachment.image())?;
	files::check_image_stamp(file, attachment.image(), attachment.source)
}

pub(super) fn detach_after(
	attachment: &Attachment,
	before_request: impl FnOnce() -> io::Result<()>,
) -> Result<DetachOutcome, Error> {
	detach_with(
		|| observe(attachment),
		before_request,
		|| protocol::detach(&attachment.device),
	)
}

fn detach_with(
	mut observe: impl FnMut() -> Result<Observation, Error>,
	before_request: impl FnOnce() -> io::Result<()>,
	request: impl FnOnce() -> Result<process::Output, process::Error>,
) -> Result<DetachOutcome, Error> {
	match observe()? {
		Observation::Detached => return Ok(DetachOutcome::AlreadyDetached),
		Observation::IdentityMismatch => return Err(Error::IdentityMismatch),
		Observation::Mounted => (),
	}

	before_request().map_err(|source| Error::DetachPrecondition { source })?;
	let outcome = request();
	match (observe(), outcome) {
		(Ok(Observation::Detached), _) => Ok(DetachOutcome::Detached),
		(Ok(Observation::Mounted), Ok(output)) if !output.status.success() => {
			if protocol::detach_busy(&output) {
				return Err(Error::DetachBusy);
			}
			Err(Error::DetachRefused {
				status: output.status,
			})
		}
		(Ok(Observation::Mounted), Err(error @ process::Error::Spawn(_))) => Err(error.into()),
		(Err(error), _) => Err(detach_uncertain(error)),
		(_, Err(error)) => Err(detach_uncertain(error.into())),
		_ => Err(detach_uncertain(Error::IdentityMismatch)),
	}
}

fn admit_path(path: &Path) -> Result<CanonicalPath, Error> {
	let canonical = CanonicalPath::try_from(path.to_path_buf())
		.map_err(|_| Error::Path { path: path.into() })?;
	files::exact_path(path)?;
	Ok(canonical)
}

fn mounted_identity(path: &Path, device: &Device) -> Result<Option<MountedIdentity>, Error> {
	HeldDirectory::open(path)?.mounted_identity(path, device)
}

fn attach_uncertain(source: Error) -> Error {
	Error::AttachUncertain {
		source: Box::new(source),
	}
}

fn detach_uncertain(source: Error) -> Error {
	Error::DetachUncertain {
		source: Box::new(source),
	}
}
