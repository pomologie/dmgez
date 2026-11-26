use super::{Arguments, rewrite_source};

fn arguments() -> Arguments {
	Arguments {
		input: Default::default(),
		output: Default::default(),
		module: Some("engine".into()),
		dependency: vec![("core_alias".into(), "local_core".into())],
		active_feature: vec!["fast-path".into()],
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
#[cfg_attr(not(feature = "unsupported-path"), inline)]
pub(in crate::module) fn run(arg: core_alias::Type) {
	let value = make!(crate::module::name, "crate::name");
	let enabled = cfg!(feature = "fast-path");
}
"#;
	let expected = r#"//! Café; crate:: and core_alias:: stay in documentation.


use {crate::internal::local_core::{self as core, Type}, crate::internal::engine::module};
use crate::internal::local_core::Other;
#[cfg(all(unix, all()))]
#[cfg_attr(not(any()), inline)]
pub(in crate::internal::engine::module) fn run(arg: crate::internal::local_core::Type) {
	let value = make!(crate::internal::engine::module::name, "crate::name");
	let enabled = cfg!(all());
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
fn rewrites_serde_path_literals_without_rewriting_names() {
	let original = r#"#[serde(serialize_with = "crate::schema::decimal", with = "::core_alias::codec", rename = "crate::display")]
value!("crate::literal");
"#;
	let expected = r#"#[serde(serialize_with = "crate :: internal :: engine :: schema :: decimal", with = "crate :: internal :: local_core :: codec", rename = "crate::display")]
value!("crate::literal");
"#;
	assert_eq!(rewrite_source(original, &arguments()).unwrap(), expected);
}

#[test]
fn requires_explicit_approval_to_remove_no_std() {
	let mut options = arguments();
	options.strip_no_std = false;
	assert!(rewrite_source("#![no_std]\n", &options).is_err());
}
