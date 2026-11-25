use serde::Serialize;
use serde_json::Serializer;

use super::{EscapedBytes, TerminalSafeJson};

#[test]
fn json_escape_retains_the_original_report_string_without_terminal_controls() {
	let original = "café \u{202e} \u{9b} \u{e0001} \x1b \" \\ \n";
	let mut output = Vec::new();
	original
		.serialize(&mut Serializer::with_formatter(
			&mut output,
			TerminalSafeJson,
		))
		.unwrap();
	assert_eq!(serde_json::from_slice::<String>(&output).unwrap(), original);
	let json = String::from_utf8(output).unwrap();
	assert!(json.contains("café"));
	for unsafe_character in ['\u{202e}', '\u{9b}', '\u{e0001}', '\x1b', '\n'] {
		assert!(!json.contains(unsafe_character), "{json:?}");
	}
}

#[test]
fn link_target_text_escapes_controls_and_non_utf8_without_replacing_bytes() {
	let target = b"dir/\x1b\xFF\\name\n";
	assert_eq!(EscapedBytes(target).to_string(), r"dir/\u{1b}\xff\\name\n");
}
