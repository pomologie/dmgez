//! Filesystem-neutral metadata admission and loss accounting (C4, D-BUILD-03).

use std::{collections::TryReserveError, mem::size_of, ops::RangeInclusive};

use dmgez_config::Metadata;
use thiserror::Error;

use super::{Attribute, Attributes, FINDER_INFO_BYTES};
use crate::manifest::{SourceIdentity, SourceMetadata, Timestamp};

mod hard_links;

pub use hard_links::HardLinks;

/// Precision provided by a target's timestamp fields.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimestampPrecision {
	Seconds,
	Nanoseconds,
}

/// Target timestamp bounds expressed in the host-neutral Unix epoch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TimestampEncoding {
	pub unix_seconds: RangeInclusive<i64>,
	pub precision: TimestampPrecision,
}

/// Tests whether a target accepts an exact attribute name and payload length.
pub type AcceptsAttribute = fn(name: &[u8], value_bytes: usize) -> bool;

/// Qualified metadata support of the filesystem adapter for the current entry kind.
#[derive(Clone, Debug, Default)]
pub struct Capabilities {
	pub permissions: bool,
	pub ownership: bool,
	pub timestamps: Option<TimestampEncoding>,
	/// Exact bit mask of BSD flags the adapter can preserve.
	pub supported_bsd_flags: u32,
	pub finder_info: bool,
	pub max_resource_fork_bytes: Option<u64>,
	pub extended_attributes: Option<AcceptsAttribute>,
	pub hard_links: bool,
}

/// What the source adapter established about the entry's attribute metadata.
#[derive(Debug)]
pub enum AttributeCapture {
	/// Complete, distinct attributes in byte order from a qualified source.
	Known(Attributes),
	/// The source adapter could not observe requested attributes.
	Unavailable,
	/// The source is generated or otherwise has no applicable attribute metadata.
	NotApplicable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimestampField {
	Accessed,
	Modified,
	Changed,
	Created,
}

/// The exact metadata field affected by a loss. Attribute values stay out of diagnostics.
#[derive(Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Field {
	Permissions,
	Ownership,
	Timestamp(TimestampField),
	BsdFlags,
	FinderInfo,
	ResourceFork { bytes: Option<u64> },
	ExtendedAttributes,
	ExtendedAttribute { name: Vec<u8>, bytes: usize },
	HardLink { identity: SourceIdentity },
}

/// Why the source field is absent or changed in the admitted metadata.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum LossReason {
	ConfiguredOmission,
	TargetUnsupported,
	SourceUnavailable,
	OptionalStatUnavailable,
	ResourceForkTooLarge { max_bytes: u64 },
	TimestampOutOfRange,
	TimestampPrecision { discarded_nanoseconds: u32 },
	UnsupportedBsdFlags { bits: u32 },
}

#[derive(Debug, Eq, PartialEq)]
pub struct Loss {
	pub field: Field,
	pub reason: LossReason,
}

/// Metadata to pass to the filesystem adapter. A missing value always stays absent.
#[derive(Debug, Default)]
pub struct AdmittedMetadata {
	pub permissions: Option<u16>,
	pub ownership: Option<(u32, u32)>,
	pub accessed: Option<Timestamp>,
	pub modified: Option<Timestamp>,
	pub changed: Option<Timestamp>,
	pub created: Option<Timestamp>,
	pub bsd_flags: Option<u32>,
	pub finder_info: Option<[u8; FINDER_INFO_BYTES]>,
	pub resource_fork: Option<Vec<u8>>,
	pub extended_attributes: Vec<Attribute>,
	pub hard_link: Option<SourceIdentity>,
}

#[derive(Debug)]
pub struct Admission {
	pub metadata: AdmittedMetadata,
	pub losses: Vec<Loss>,
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
	#[error("required source metadata could not be preserved ({0:?})")]
	Loss(Loss),
	#[error("metadata policy exceeds its remaining byte budget")]
	Budget,
	#[error("could not reserve a bounded metadata policy buffer")]
	Allocation(#[from] TryReserveError),
}

/// Configured field requests and the effective strict/report setting.
pub struct Policy<'config> {
	requests: &'config Metadata,
	fail_on_loss: bool,
}

impl<'config> Policy<'config> {
	/// Use the project's `Build::fail_on_metadata_loss` result as `fail_on_loss`.
	#[must_use]
	pub const fn new(requests: &'config Metadata, fail_on_loss: bool) -> Self {
		Self {
			requests,
			fail_on_loss,
		}
	}

	/// Admit each captured field from one entry, retaining exact opaque values.
	/// Whole-second targets floor fractional Unix timestamps in report mode and
	/// disclose the discarded precision. Optional stat fields missing from the
	/// current host and explicit preservation opt-outs are recorded in either mode.
	///
	/// Charge retained report metadata to `remaining_report_bytes`. The caller
	/// charges captured attributes to its separate capture budget and associates
	/// the result with the entry's virtual path. This method opens no handles.
	///
	/// # Errors
	/// Returns the first required loss in strict mode or an allocation/budget
	/// failure. On error the supplied report budget is left unchanged.
	pub fn admit(
		&self,
		source: &SourceMetadata,
		attributes: AttributeCapture,
		hard_link: Option<SourceIdentity>,
		capabilities: &Capabilities,
		remaining_report_bytes: &mut usize,
	) -> Result<Admission, Error> {
		let mut report = Report {
			strict: self.fail_on_loss,
			remaining: *remaining_report_bytes,
			losses: Vec::new(),
		};
		let requests = self.requests;
		let mut metadata = AdmittedMetadata {
			permissions: report.retain_stat(
				Field::Permissions,
				source.permissions,
				requests.preserve_permissions,
				capabilities.permissions,
			)?,
			ownership: report.retain_stat(
				Field::Ownership,
				source.ownership,
				requests.preserve_ownership,
				capabilities.ownership,
			)?,
			..AdmittedMetadata::default()
		};
		let encoding = capabilities.timestamps.as_ref();
		for (field, observed, admitted) in [
			(
				TimestampField::Accessed,
				source.accessed,
				&mut metadata.accessed,
			),
			(
				TimestampField::Modified,
				source.modified,
				&mut metadata.modified,
			),
			(
				TimestampField::Changed,
				source.changed,
				&mut metadata.changed,
			),
			(
				TimestampField::Created,
				source.created,
				&mut metadata.created,
			),
		] {
			*admitted =
				report.timestamp(field, observed, requests.preserve_timestamps, encoding)?;
		}
		metadata.bsd_flags = match source.flags {
			Some(bits) => {
				let unsupported = bits & !capabilities.supported_bsd_flags;
				if unsupported != 0 {
					report.add(
						Field::BsdFlags,
						LossReason::UnsupportedBsdFlags { bits: unsupported },
					)?;
				}
				Some(bits & capabilities.supported_bsd_flags)
			}
			None => {
				report.add(Field::BsdFlags, LossReason::OptionalStatUnavailable)?;
				None
			}
		};
		if let Some(identity) = hard_link {
			metadata.hard_link = report.retain(
				Field::HardLink { identity },
				identity,
				requests.preserve_hard_links,
				capabilities.hard_links,
			)?;
		}
		match attributes {
			AttributeCapture::Known(attributes) => {
				self.admit_attributes(attributes, capabilities, &mut metadata, &mut report)?;
			}
			AttributeCapture::Unavailable => {
				for (requested, field) in [
					(requests.preserve_finder_info, Field::FinderInfo),
					(
						requests.preserve_resource_forks,
						Field::ResourceFork { bytes: None },
					),
					(
						requests.preserve_extended_attributes,
						Field::ExtendedAttributes,
					),
				] {
					if requested {
						report.add(field, LossReason::SourceUnavailable)?;
					}
				}
			}
			AttributeCapture::NotApplicable => {}
		}
		*remaining_report_bytes = report.remaining;
		Ok(Admission {
			metadata,
			losses: report.losses,
		})
	}

	fn admit_attributes(
		&self,
		attributes: Attributes,
		capabilities: &Capabilities,
		metadata: &mut AdmittedMetadata,
		report: &mut Report,
	) -> Result<(), Error> {
		if let Some(info) = attributes.finder_info {
			metadata.finder_info = report.retain(
				Field::FinderInfo,
				info,
				self.requests.preserve_finder_info,
				capabilities.finder_info,
			)?;
		}
		if let Some(fork) = attributes.resource_fork {
			let bytes = u64::try_from(fork.len()).map_err(|_| Error::Budget)?;
			let field = Field::ResourceFork { bytes: Some(bytes) };
			match (
				self.requests.preserve_resource_forks,
				capabilities.max_resource_fork_bytes,
			) {
				(true, Some(max_bytes)) if bytes > max_bytes => {
					report.add(field, LossReason::ResourceForkTooLarge { max_bytes })?;
				}
				(requested, supported) => {
					metadata.resource_fork =
						report.retain(field, fork, requested, supported.is_some())?;
				}
			}
		}
		// Reuse the original bounded attribute vector as the retained output.
		let mut extended = attributes.extended;
		let mut loss = Ok(());
		extended.retain_mut(|attribute| {
			if loss.is_err() {
				return false;
			}
			let reason = if !self.requests.preserve_extended_attributes {
				LossReason::ConfiguredOmission
			} else if capabilities
				.extended_attributes
				.is_some_and(|accepts| accepts(&attribute.name, attribute.value.len()))
			{
				return true;
			} else {
				LossReason::TargetUnsupported
			};
			loss = report.add(
				Field::ExtendedAttribute {
					name: std::mem::take(&mut attribute.name),
					bytes: attribute.value.len(),
				},
				reason,
			);
			false
		});
		loss?;
		metadata.extended_attributes = extended;
		Ok(())
	}
}

struct Report {
	strict: bool,
	remaining: usize,
	losses: Vec<Loss>,
}

impl Report {
	fn retain_stat<T>(
		&mut self,
		field: Field,
		value: Option<T>,
		requested: bool,
		supported: bool,
	) -> Result<Option<T>, Error> {
		if let Some(value) = value {
			return self.retain(field, value, requested, supported);
		}
		let reason = if requested {
			LossReason::SourceUnavailable
		} else {
			LossReason::ConfiguredOmission
		};
		self.add(field, reason)?;
		Ok(None)
	}

	fn retain<T>(
		&mut self,
		field: Field,
		value: T,
		requested: bool,
		supported: bool,
	) -> Result<Option<T>, Error> {
		if !requested {
			self.add(field, LossReason::ConfiguredOmission)?;
		} else if !supported {
			self.add(field, LossReason::TargetUnsupported)?;
		} else {
			return Ok(Some(value));
		}
		Ok(None)
	}

	fn timestamp(
		&mut self,
		kind: TimestampField,
		value: Option<Timestamp>,
		requested: bool,
		encoding: Option<&TimestampEncoding>,
	) -> Result<Option<Timestamp>, Error> {
		let field = Field::Timestamp(kind);
		let Some(mut timestamp) = value else {
			if requested {
				self.add(field, LossReason::OptionalStatUnavailable)?;
			}
			return Ok(None);
		};
		let Some(encoding) = encoding.filter(|_| requested) else {
			return self.retain(field, timestamp, requested, false);
		};
		if !encoding.unix_seconds.contains(&timestamp.seconds) {
			self.add(field, LossReason::TimestampOutOfRange)?;
			return Ok(None);
		}
		if encoding.precision == TimestampPrecision::Seconds && timestamp.nanoseconds != 0 {
			self.add(
				field,
				LossReason::TimestampPrecision {
					discarded_nanoseconds: timestamp.nanoseconds,
				},
			)?;
			timestamp.nanoseconds = 0;
		}
		Ok(Some(timestamp))
	}

	fn add(&mut self, field: Field, reason: LossReason) -> Result<(), Error> {
		let optional = matches!(
			reason,
			LossReason::ConfiguredOmission | LossReason::OptionalStatUnavailable
		);
		if self.strict && !optional {
			return Err(Error::Loss(Loss { field, reason }));
		}
		let name_bytes = match &field {
			Field::ExtendedAttribute { name, .. } => name.capacity(),
			_ => 0,
		};
		let remaining = self
			.remaining
			.checked_sub(name_bytes)
			.ok_or(Error::Budget)?;
		let old_capacity = self.losses.capacity();
		if self.losses.len() == old_capacity {
			let available = remaining / size_of::<Loss>();
			let additional = old_capacity.max(1).min(available);
			if additional == 0 {
				return Err(Error::Budget);
			}
			self.losses.try_reserve_exact(additional)?;
		}
		let allocated = (self.losses.capacity() - old_capacity)
			.checked_mul(size_of::<Loss>())
			.ok_or(Error::Budget)?;
		self.remaining = remaining.checked_sub(allocated).ok_or(Error::Budget)?;
		self.losses.push(Loss { field, reason });
		Ok(())
	}
}

#[cfg(test)]
mod tests;
