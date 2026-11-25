use dmgez_vfs::volume::FINDER_INFO_BYTES;
use serde::Serialize;

#[cfg(feature = "finder-layout")]
use super::finder_layout::{COMPARED_WITH_LAYOUT, COMPARED_WITH_LAYOUT_AND_XATTRS};
use super::{
	COMPARED_FIELDS, COMPARED_FIELDS_WITH_XATTRS, Field, Policy, UNCOMPARED_FIELDS,
	UNCOMPARED_FIELDS_WITH_XATTRS, Uncompared, replace_finder_exclusion, with_field,
};

const COMPARED_WITH_FINDER_INFO: [Field; COMPARED_FIELDS.len() + 1] =
	with_field(&COMPARED_FIELDS, Field::FinderInfo);
const COMPARED_WITH_FINDER_INFO_AND_XATTRS: [Field; COMPARED_FIELDS_WITH_XATTRS.len() + 1] =
	with_field(&COMPARED_FIELDS_WITH_XATTRS, Field::FinderInfo);
const EXCLUDED_WITH_FINDER_INFO: [Uncompared; UNCOMPARED_FIELDS.len()] =
	replace_finder_exclusion(UNCOMPARED_FIELDS, Uncompared::FinderLayout);
const EXCLUDED_WITH_FINDER_INFO_AND_XATTRS: [Uncompared; UNCOMPARED_FIELDS_WITH_XATTRS.len()] =
	replace_finder_exclusion(UNCOMPARED_FIELDS_WITH_XATTRS, Uncompared::FinderLayout);

#[cfg(feature = "finder-layout")]
const COMPARED_WITH_BOTH_FINDER_SCOPES: [Field; COMPARED_WITH_LAYOUT.len() + 1] =
	with_field(&COMPARED_WITH_LAYOUT, Field::FinderInfo);
#[cfg(feature = "finder-layout")]
const COMPARED_WITH_BOTH_FINDER_SCOPES_AND_XATTRS: [Field;
	COMPARED_WITH_LAYOUT_AND_XATTRS.len() + 1] =
	with_field(&COMPARED_WITH_LAYOUT_AND_XATTRS, Field::FinderInfo);
#[cfg(feature = "finder-layout")]
const EXCLUDED_WITH_BOTH_FINDER_SCOPES: [Uncompared; UNCOMPARED_FIELDS.len() - 1] =
	without_finder_exclusion(&UNCOMPARED_FIELDS);
#[cfg(feature = "finder-layout")]
const EXCLUDED_WITH_BOTH_FINDER_SCOPES_AND_XATTRS: [Uncompared;
	UNCOMPARED_FIELDS_WITH_XATTRS.len() - 1] =
	without_finder_exclusion(&UNCOMPARED_FIELDS_WITH_XATTRS);

#[cfg(feature = "finder-layout")]
const fn without_finder_exclusion<const N: usize>(base: &[Uncompared]) -> [Uncompared; N] {
	let mut result = [Uncompared::FinderMetadata; N];
	let mut source = 0;
	let mut target = 0;
	while source < base.len() {
		let field = base[source];
		if !matches!(field, Uncompared::FinderMetadata) {
			result[target] = field;
			target += 1;
		}
		source += 1;
	}
	assert!(target == N);
	result
}

/// How effective FinderInfo on common objects is compared.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum FinderInfoComparison {
	/// Compare exact backend-admitted bytes and established record absence.
	ExactEffectiveBytes,
}

/// Scope of per-object FinderInfo in the report.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct FinderInfoPolicy {
	pub comparison: FinderInfoComparison,
	/// Length of a present FinderInfo record in bytes.
	pub record_bytes: usize,
	/// Whether FinderInfo is compared only when the two objects have the same kind.
	pub same_kind_only: bool,
}

impl Policy {
	pub(super) fn with_finder_info(mut self) -> Self {
		self.finder_info = Some(FinderInfoPolicy {
			comparison: FinderInfoComparison::ExactEffectiveBytes,
			record_bytes: FINDER_INFO_BYTES,
			same_kind_only: true,
		});
		let (compared, excluded): (&[Field], &[Uncompared]) = if self.xattrs.is_some() {
			(
				&COMPARED_WITH_FINDER_INFO_AND_XATTRS,
				&EXCLUDED_WITH_FINDER_INFO_AND_XATTRS,
			)
		} else {
			(&COMPARED_WITH_FINDER_INFO, &EXCLUDED_WITH_FINDER_INFO)
		};
		#[cfg(feature = "finder-layout")]
		let (compared, excluded): (&[Field], &[Uncompared]) = if self.finder_layout.is_some() {
			if self.xattrs.is_some() {
				(
					&COMPARED_WITH_BOTH_FINDER_SCOPES_AND_XATTRS,
					&EXCLUDED_WITH_BOTH_FINDER_SCOPES_AND_XATTRS,
				)
			} else {
				(
					&COMPARED_WITH_BOTH_FINDER_SCOPES,
					&EXCLUDED_WITH_BOTH_FINDER_SCOPES,
				)
			}
		} else {
			(compared, excluded)
		};
		self.compared = compared;
		self.excluded = excluded;
		self
	}
}
