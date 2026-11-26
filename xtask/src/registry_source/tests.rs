use super::{Arguments, rewrite_source};

fn arguments() -> Arguments {
	Arguments {
		input: Default::default(),
		output: Default::default(),
		module: Some("engine".into()),
		dependency: vec![("core_alias".into(), "local_core".into())],
		feature_prefix: Some("bundle_engine_".into()),
		strip_no_std: true,
	}
}

#[test]
fn rewrites_paths_and_conditions_without_reformatting_the_source() {
	let original = r#"//! Café; crate:: and core_alias:: stay in documentation.
#![no_std]

use {core_alias::{self as core, Type}, crate::module};
use ::core_alias::Other;
#[cfg(all(unix, feature = "fast-path"))]
pub(in crate::module) fn run(arg: core_alias::Type) {
	let value = make!(crate::module::name, "crate::name");
	let enabled = cfg!(feature = "fast-path");
}
"#;
	let expected = r#"//! Café; crate:: and core_alias:: stay in documentation.


use {crate::internal::local_core::{self as core, Type}, crate::internal::engine::module};
use crate::internal::local_core::Other;
#[cfg(all(unix, feature = "bundle_engine_fast-path"))]
pub(in crate::internal::engine::module) fn run(arg: crate::internal::local_core::Type) {
	let value = make!(crate::internal::engine::module::name, "crate::name");
	let enabled = cfg!(feature = "bundle_engine_fast-path");
}
"#;
	assert_eq!(rewrite_source(original, &arguments()).unwrap(), expected);
}

#[test]
fn preserves_the_application_crate_root() {
	let mut options = arguments();
	options.module = None;
	assert_eq!(
		rewrite_source("use {crate::command, core_alias::Type};", &options).unwrap(),
		"use {crate::command, crate::internal::local_core::Type};"
	);
}

#[test]
fn requires_explicit_approval_to_remove_no_std() {
	let mut options = arguments();
	options.strip_no_std = false;
	assert!(rewrite_source("#![no_std]\n", &options).is_err());
}
