//! Bounded inspection of thin and universal Mach-O files.

mod inspection;

pub use inspection::{
	Architecture, Classification, CodeKind, architectures, classify, has_or_may_have_signature_at,
};
