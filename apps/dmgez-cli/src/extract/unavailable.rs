use std::fmt;

#[derive(Debug)]
pub(super) struct Error {
	feature: &'static str,
	compiled: bool,
}

impl Error {
	pub(super) const fn new(feature: &'static str, compiled: bool) -> Self {
		Self { feature, compiled }
	}
}

impl fmt::Display for Error {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		if self.compiled {
			formatter.write_str("host extraction requires macOS or Linux")
		} else {
			write!(formatter, "feature not compiled: {}", self.feature)
		}
	}
}

impl std::error::Error for Error {}
