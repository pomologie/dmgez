use std::collections::BTreeMap;

use serde::Deserialize;

use crate::{LocalPath, Text, VirtualPath};

/// Backend selection recognized by a release configuration.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[non_exhaustive]
#[serde(rename_all = "lowercase")]
pub enum Backend {
	#[default]
	Auto,
	Rust,
	Native,
}

/// Requested treatment of app signing.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[non_exhaustive]
#[serde(rename_all = "lowercase")]
pub enum AppSigning {
	Verify,
	Preserve,
	Sign,
}

/// The authentication policy for a requested signing operation.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[non_exhaustive]
#[serde(rename_all = "kebab-case")]
pub enum SigningMode {
	/// Authenticate the signature with a Developer ID Application identity.
	#[default]
	DeveloperId,
	/// Seal the artifact without a signing identity.
	AdHoc,
}

/// Signing configuration. Credential references name credentials supplied at execution.
#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
#[serde(deny_unknown_fields)]
pub struct Signing {
	pub enabled: Option<bool>,
	pub backend: Option<Backend>,
	/// Signing policy. Omission selects Developer ID for existing manifests.
	#[serde(default)]
	pub mode: SigningMode,
	pub apps: Option<AppSigning>,
	pub dmg: Option<bool>,
	/// Code-signing identifier for the disk image.
	pub identifier: Option<Text>,
	pub identity: Option<Text>,
	pub credential_ref: Option<Text>,
	#[serde(default)]
	pub rules: Vec<SigningRule>,
}

/// An entitlements file explicitly attached to an image path.
#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
#[serde(deny_unknown_fields)]
pub struct SigningRule {
	pub path: VirtualPath,
	pub entitlements: LocalPath,
}

/// Configuration for notarizing a release artifact.
#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
#[serde(deny_unknown_fields)]
pub struct Notarization {
	pub enabled: Option<bool>,
	pub backend: Option<Backend>,
	pub wait: Option<bool>,
	pub staple: Option<bool>,
	pub credential_ref: Option<Text>,
}

/// Explicit verification requirements for a release.
#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
#[serde(deny_unknown_fields)]
pub struct Release {
	pub verify: Option<bool>,
	pub require_notarization: Option<bool>,
	pub require_native_assessment: Option<bool>,
}

/// Localized license files and optional button text.
#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
#[serde(deny_unknown_fields)]
pub struct License {
	pub default_language: Text,
	pub licenses: BTreeMap<String, LocalPath>,
	#[serde(default)]
	pub buttons: BTreeMap<String, Buttons>,
}

/// User-supplied text for localized license controls.
#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
#[serde(deny_unknown_fields)]
pub struct Buttons {
	pub agree: Option<Text>,
	pub disagree: Option<Text>,
	pub print: Option<Text>,
	pub save: Option<Text>,
	pub language: Option<Text>,
	pub message: Option<Text>,
}
