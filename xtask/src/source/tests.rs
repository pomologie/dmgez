use std::fs;

use super::{MAX_LINES, check, check_entry};

#[test]
fn permits_documentation_attributes_declarations_and_reexports() {
	let source = r###"//! Crate documentation
#![no_std]
#![cfg_attr(not(feature = "std"), forbid(unsafe_code))]
#![doc = include_str!("../README.md")]
/* A /* nested */ block comment. */
#[cfg(kani)]
#[path = "../verification/kani/proof.rs"]
mod proof;
#[cfg_attr(all(feature = "docs", unix),
    doc = r#"A comma, a bracket ], and a quote " in documentation."#,
    cfg_attr(unix, cfg(feature = "api"), doc(hidden)),
)]
pub(crate) mod api;
pub use api::{Service, Read as Reader, r#type};
pub(in crate::scope) use api::Path;
"###;
	assert_eq!(check_entry(source), Ok(()));
}

#[test]
fn rejects_definitions_even_when_disabled() {
	let declarations = [
		"struct Record;",
		"enum Kind { One }",
		"trait Read {}",
		"impl Record { fn read() {} }",
		"fn read() {}",
		"const PAGE_SIZE: usize = 4096;",
		"static COUNTER: usize = 0;",
		"type Id = u64;",
		"mod tests { #[test] fn reads() {} }",
		r#"include!("generated.rs");"#,
		"use api::Service;",
	];
	for declaration in declarations {
		let source = format!("mod api;\n#[cfg(any())]\n{declaration}");
		assert!(
			check_entry(&source).unwrap_err().contains("named module"),
			"{declaration}"
		);
	}
}

#[test]
fn rejects_code_generating_outer_attributes_with_locations() {
	let attributes = [
		"#[generate]",
		"#[macros::generate]",
		"#[cfg_attr(unix, generate)]",
		"#[cfg_attr(unix, doc(hidden), cfg_attr(unix, macros::generate))]",
		"#[cfg_attr(unix, cfg_attr(unix, doc(hidden)), generate)]",
	];
	for attribute in attributes {
		for declaration in ["pub mod api;", "pub use api::Service;"] {
			let source = format!("//! Crate documentation\n{attribute}\n{declaration}");
			let error = check_entry(&source).unwrap_err();
			assert!(error.starts_with("2:1:"), "{error}");
		}
	}
}

#[test]
fn rejects_definitions_in_attribute_expressions_and_malformed_rust() {
	let attributes = [
		r#"#[cfg_attr(any(), path = { fn read() {} "api.rs" })]"#,
		r#"#![cfg_attr(any(), doc = { fn read() {} "documentation" })]"#,
	];
	for attribute in attributes {
		let source = format!("{attribute}\nmod api;");
		assert!(
			check_entry(&source)
				.unwrap_err()
				.contains("structural attributes")
		);
	}
	assert!(
		check_entry("pub use {")
			.unwrap_err()
			.contains("invalid Rust syntax")
	);
}

#[test]
fn scans_excluded_sources_and_ignores_the_cargo_target_directory() {
	let directory = tempfile::tempdir().unwrap();
	let root = directory.path();
	let target = root.join("generated");
	for path in [
		root.join("crates/standalone"),
		root.join("xtask"),
		target.clone(),
	] {
		fs::create_dir_all(path).unwrap();
	}
	fs::write(target.join("mod.rs"), "fn generated() {}").unwrap();
	fs::write(root.join("crates/standalone/lib.rs"), "mod code;\n").unwrap();
	assert_eq!(check(root, &target).unwrap(), 1);

	fs::write(root.join("xtask/many.rs"), "\n".repeat(MAX_LINES + 1)).unwrap();
	fs::write(
		root.join("crates/standalone/lib.rs"),
		"mod code;\n    pub fn bad() {}",
	)
	.unwrap();
	let errors = check(root, &target).unwrap_err().to_string();
	assert!(errors.contains("lib.rs:2:5: move this item"));
	assert!(errors.contains("many.rs exceeds 500 lines"));
}
