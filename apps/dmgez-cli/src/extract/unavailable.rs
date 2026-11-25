use std::fmt;

use super::FEATURE;

#[derive(Debug)]
pub(super) struct Error;

impl fmt::Display for Error {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		if cfg!(feature = "hfsplus-extract") {
			formatter.write_str("host extraction requires macOS or Linux")
		} else {
			write!(formatter, "feature not compiled: {FEATURE}")
		}
	}
}

impl std::error::Error for Error {}
