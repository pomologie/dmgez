use std::{
	fmt,
	io::{self, Write},
};

use serde::Serialize;
use serde_json::{Serializer, ser::Formatter};

use crate::CliResult;

pub(super) fn write_report(
	json: bool,
	report: &impl Serialize,
	text: impl FnOnce(&mut io::StdoutLock<'_>) -> io::Result<()>,
) -> CliResult {
	let mut output = io::stdout().lock();
	if json {
		write_json(&mut output, report)?;
		writeln!(output)?;
	} else {
		text(&mut output)?;
	}
	output.flush()?;
	Ok(())
}

pub(super) fn write_json(
	output: &mut impl Write,
	value: &impl Serialize,
) -> serde_json::Result<()> {
	value.serialize(&mut Serializer::with_formatter(output, TerminalSafeJson))
}

pub(super) struct EscapedText<'a>(pub &'a str);

impl fmt::Debug for EscapedText<'_> {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		fmt::Debug::fmt(self.0, formatter)
	}
}

impl fmt::Display for EscapedText<'_> {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(formatter, "{}", self.0.escape_debug())
	}
}

#[cfg(any(feature = "hfsplus-read", feature = "apfs-read", test))]
pub(super) struct EscapedBytes<'a>(pub &'a [u8]);

#[cfg(any(feature = "hfsplus-read", feature = "apfs-read", test))]
impl fmt::Display for EscapedBytes<'_> {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		for chunk in self.0.utf8_chunks() {
			write!(formatter, "{}", EscapedText(chunk.valid()))?;
			for byte in chunk.invalid() {
				write!(formatter, "{}", byte.escape_ascii())?;
			}
		}
		Ok(())
	}
}

struct TerminalSafeJson;

impl Formatter for TerminalSafeJson {
	fn write_string_fragment<W: ?Sized + Write>(
		&mut self,
		writer: &mut W,
		fragment: &str,
	) -> io::Result<()> {
		let mut start = 0;
		for (index, character) in fragment.char_indices() {
			// Serde already handles JSON's mandatory escapes. Escape the remaining
			// invisible characters so a report is also safe to display verbatim.
			if character.escape_debug().next() != Some(character) {
				writer.write_all(&fragment.as_bytes()[start..index])?;
				for unit in character.encode_utf16(&mut [0; 2]) {
					write!(writer, "\\u{unit:04x}")?;
				}
				start = index + character.len_utf8();
			}
		}
		writer.write_all(&fragment.as_bytes()[start..])
	}
}

#[cfg(test)]
mod tests;
