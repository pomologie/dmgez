//! Structural checks of a selected HFS+ or HFSX volume.

mod error;
mod verification;

pub use error::{CheckError, CheckFailureKind};
pub use hfsplus_reader::check::StructuralReport;
pub use verification::check;
