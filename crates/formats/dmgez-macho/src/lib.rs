//! Bounded inspection of thin and universal Mach-O files.

pub mod bundle;
mod inspection;

pub use inspection::{
	Architecture, Classification, CodeKind, architectures, classify, has_or_may_have_signature_at,
};
