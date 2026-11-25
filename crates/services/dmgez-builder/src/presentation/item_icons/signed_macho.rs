use dmgez_core::source::{CapturedFile, SourceError};

pub(super) fn has_or_may_have_signature(source: &CapturedFile) -> Result<bool, SourceError> {
	dmgez_macho::has_or_may_have_signature_at(source.len(), |offset, output| {
		source.read_exact_at(offset, output)
	})
}
