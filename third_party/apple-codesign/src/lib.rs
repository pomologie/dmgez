// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Apple code-signature primitives for the vendored offline DMG profile.

#![forbid(unsafe_code)]

mod code_directory;
pub use code_directory::*;
pub mod code_requirement;
pub use code_requirement::*;
pub mod cryptography;
pub mod dmg;
pub mod embedded_signature;
pub use embedded_signature::*;
pub mod embedded_signature_builder;
pub mod environment_constraints;
mod error;
pub use error::*;
pub mod plist_der;
