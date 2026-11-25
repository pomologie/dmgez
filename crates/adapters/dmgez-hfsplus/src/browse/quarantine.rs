use hfsplus_reader::catalog::{AttributePresence, AttributeScope};

use super::{CatalogId, Error, HfsBackend, LogicalDisk, Quarantine, QuarantineScope};

const QUARANTINE_ATTRIBUTE: &str = "com.apple.quarantine";

impl<S: LogicalDisk> HfsBackend<'_, '_, S> {
	pub(super) fn check_quarantine(
		&self,
		node: CatalogId,
		scope: QuarantineScope,
	) -> Result<Quarantine, Error> {
		let name: [u16; QUARANTINE_ATTRIBUTE.len()] =
			std::array::from_fn(|index| u16::from(QUARANTINE_ATTRIBUTE.as_bytes()[index]));
		let scope = if scope == QuarantineScope::Object {
			AttributeScope::Entry
		} else {
			AttributeScope::EntryAndAncestors
		};
		let presence = self.catalog.observe_entry_attribute(
			node,
			&name,
			scope,
			self.limits.hard_links,
			self.limits.logical_data.attributes,
		)?;
		Ok(match presence {
			AttributePresence::Present => Quarantine::Marked,
			AttributePresence::Absent => Quarantine::Clear,
			_ => Quarantine::Unavailable,
		})
	}
}
