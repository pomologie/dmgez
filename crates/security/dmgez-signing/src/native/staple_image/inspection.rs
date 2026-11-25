use std::{os::unix::fs::FileExt, path::Path};

use dmgez_udif::{has_stapled_ticket, signed_signature_range, validate_unsigned_signature_fields};

use crate::notarization::TicketStatus;

use super::{Artifact, Error, image, open_image, snapshot::signature_buffer, stapler};

// Apple's stapler(1) distinguishes invalid data and revoked tickets from
// ambiguous process failures, including unexpected ticket service responses.
const EX_DATAERR: i32 = 65;
const EX_NOPERM: i32 = 77;

pub(crate) fn check_ticket(image: &Path) -> Result<TicketStatus, crate::notarization::Error> {
	open_image(image)
		.and_then(|image| check(&image, |path| stapler::Tool::new()?.validate(path)))
		.map_err(|source| crate::notarization::Error::TicketValidation(Box::new(source)))
}

pub(super) fn check(
	image: &Artifact,
	validate: impl FnOnce(&Path) -> Result<(), stapler::Error>,
) -> Result<TicketStatus, Error> {
	image.validate()?;
	let present = ticket_present(image);
	image.validate()?;
	if !present? {
		return Ok(TicketStatus::Missing);
	}
	let result = validate(image.path());
	image.validate()?;
	match result {
		Ok(()) => Ok(TicketStatus::Verified),
		Err(stapler::Error::Unavailable) => Ok(TicketStatus::Unavailable),
		Err(stapler::Error::Validate(status))
			if matches!(status.code(), Some(EX_DATAERR | EX_NOPERM)) =>
		{
			Ok(TicketStatus::Invalid)
		}
		Err(source) => Err(source.into()),
	}
}

fn ticket_present(image: &Artifact) -> Result<bool, Error> {
	let trailer = image::read_trailer(image.file())?;
	if validate_unsigned_signature_fields(&trailer).is_ok() {
		return Ok(false);
	}
	let range = signed_signature_range(&trailer, image.len())?;
	let mut signature = signature_buffer(&range)?;
	image.file().read_exact_at(&mut signature, range.start)?;
	Ok(has_stapled_ticket(&signature)?)
}
