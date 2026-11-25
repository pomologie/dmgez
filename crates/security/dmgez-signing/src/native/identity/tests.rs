use super::{Error, developer_id_available, listed_developer_id};

const SELECTED: &str = "0123456789ABCDEF0123456789ABCDEF01234567";
const DIFFERENT: &str = "ABCDEF0123456789ABCDEF0123456789ABCDEF01";

#[test]
fn finds_the_exact_fingerprint_and_developer_id_application_label() {
	let listing = format!(
		"  1) {DIFFERENT} \"Apple Development: Sample (EXAMPLE001)\"\n  2) {SELECTED} \"Developer ID Application: Sample \"Quoted\" (EXAMPLE001)\"\n     2 valid identities found\n"
	);
	assert!(listed_developer_id(listing.as_bytes(), &SELECTED.to_ascii_lowercase()).unwrap());
	assert!(!listed_developer_id(listing.as_bytes(), DIFFERENT).unwrap());
}

#[test]
fn refuses_missing_wrong_kind_and_annotated_identities() {
	assert!(!listed_developer_id(b"     0 valid identities found\n", SELECTED).unwrap());
	for identity in [
		"\"Developer ID Installer: Sample (EXAMPLE001)\"",
		"\"Developer ID Application: Sample (EXAMPLE001)\" (CSSMERR_TP_CERT_EXPIRED)",
		"\"Developer ID Application: \"",
		"\"Unrelated Developer ID Application: Sample (EXAMPLE001)\"",
	] {
		let listing = format!("  1) {SELECTED} {identity}\n     1 valid identities found\n");
		assert!(!listed_developer_id(listing.as_bytes(), SELECTED).unwrap());
	}
}

#[test]
fn rejects_malformed_listings_without_recording_their_contents() {
	let selected = format!("  1) {SELECTED} \"Developer ID Application: Sample (EXAMPLE001)\"\n");
	for listing in [
		selected.clone(),
		format!("{selected}     0 valid identities found\n"),
		format!(
			"{selected}  1) {DIFFERENT} \"Developer ID Application: Embedded (EXAMPLE001)\"\n     2 valid identities found\n"
		),
		format!(
			"  1) {SELECTED} \"Developer ID Application: Sample\nLabel\"\n     1 valid identities found\n"
		),
	] {
		let error = listed_developer_id(listing.as_bytes(), SELECTED).unwrap_err();
		assert!(matches!(error, Error::MalformedOutput));
		assert!(!error.to_string().contains("Sample"));
		assert!(!format!("{error:?}").contains(SELECTED));
	}
}

#[test]
fn rejects_invalid_fingerprints_before_keychain_lookup() {
	for malformed in ["-", "", "0123456789ABCDEF0123456789ABCDEF0123456G"] {
		assert!(matches!(
			developer_id_available(malformed),
			Err(Error::Fingerprint)
		));
	}
}
