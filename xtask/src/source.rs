use std::{
	error::Error,
	fs,
	path::{Path, PathBuf},
};

use proc_macro2::{Delimiter, Span, TokenStream, TokenTree};
use quote::ToTokens;
use syn::{
	Attribute, File, Item, MacroDelimiter, Meta, Token, Visibility, punctuated::Punctuated,
	spanned::Spanned,
};

#[cfg(test)]
mod tests;

const MAX_LINES: usize = 500;
const ROOT_NAMES: &[&str] = &["lib.rs", "mod.rs"];
const SIZE_SCOPES: &[&str] = &["apps", "crates", "fuzz", "xtask"];
const IGNORED_DIR_NAMES: &[&str] = &[".git", "target"];
const IGNORED_PATHS: &[&str] = &["docs/temp", "fuzz/artifacts", "fuzz/corpus"];

pub(crate) fn check(root: &Path, target: &Path) -> Result<usize, Box<dyn Error>> {
	let mut files = rust_files(root, target)?;
	files.sort_unstable();
	let mut roots = 0;
	let mut errors = Vec::new();
	for path in files {
		let relative = path.strip_prefix(root)?;
		let entry_point = path
			.file_name()
			.and_then(|name| name.to_str())
			.is_some_and(|name| ROOT_NAMES.contains(&name));
		let check_size = SIZE_SCOPES.iter().any(|scope| relative.starts_with(scope));
		if !entry_point && !check_size {
			continue;
		}
		let source = fs::read_to_string(&path)
			.map_err(|error| format!("{}: {error}", relative.display()))?;
		if check_size && source.lines().count() > MAX_LINES {
			errors.push(format!("{} exceeds {MAX_LINES} lines", relative.display()));
		}
		if entry_point {
			roots += 1;
			if let Err(error) = check_entry(&source) {
				errors.push(format!("{}:{error}", relative.display()));
			}
		}
	}
	if errors.is_empty() {
		Ok(roots)
	} else {
		Err(errors.join("\n").into())
	}
}

fn rust_files(root: &Path, target: &Path) -> Result<Vec<PathBuf>, Box<dyn Error>> {
	let mut files = Vec::new();
	let mut pending = vec![root.to_owned()];
	while let Some(directory) = pending.pop() {
		for entry in fs::read_dir(&directory)? {
			let entry = entry?;
			let kind = entry.file_type()?;
			let path = entry.path();
			if kind.is_dir() {
				if path != target
					&& !IGNORED_DIR_NAMES
						.iter()
						.any(|name| entry.file_name() == *name)
					&& !IGNORED_PATHS.iter().any(|name| path == root.join(name))
				{
					pending.push(path);
				}
			} else if path.extension().is_some_and(|extension| extension == "rs")
				&& (kind.is_file() || (kind.is_symlink() && path.is_file()))
			{
				files.push(path);
			}
		}
	}
	Ok(files)
}

fn check_entry(source: &str) -> Result<(), String> {
	let file: File = syn::parse_file(source)
		.map_err(|error| diagnostic(error.span(), &format!("invalid Rust syntax: {error}")))?;
	if file.shebang.is_some() {
		return Err("1:1: executable header in a structural entry point".into());
	}
	for attribute in &file.attrs {
		if has_braces(attribute.meta.to_token_stream()) {
			return Err(attribute_error(attribute));
		}
	}
	for item in file.items {
		let attributes = match &item {
			Item::Mod(module) if module.content.is_none() => &module.attrs,
			Item::Use(reexport) if !matches!(reexport.vis, Visibility::Inherited) => {
				&reexport.attrs
			}
			_ => {
				return Err(diagnostic(
					item.span(),
					"move this item into a named module",
				));
			}
		};
		for attribute in attributes {
			if has_braces(attribute.meta.to_token_stream()) || !structural(&attribute.meta) {
				return Err(attribute_error(attribute));
			}
		}
	}
	Ok(())
}

fn structural(meta: &Meta) -> bool {
	match meta {
		Meta::List(list) if matches!(list.delimiter, MacroDelimiter::Paren(_)) => {
			if list.path.is_ident("cfg_attr") {
				list.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
					.is_ok_and(|arguments| {
						let mut arguments = arguments.iter();
						arguments.next().is_some() && arguments.all(structural)
					})
			} else {
				list.path.is_ident("cfg") || list.path.is_ident("doc")
			}
		}
		Meta::NameValue(value) => value.path.is_ident("doc") || value.path.is_ident("path"),
		_ => false,
	}
}

fn has_braces(tokens: TokenStream) -> bool {
	tokens.into_iter().any(|token| match token {
		TokenTree::Group(group) => {
			group.delimiter() == Delimiter::Brace || has_braces(group.stream())
		}
		_ => false,
	})
}

fn attribute_error(attribute: &Attribute) -> String {
	diagnostic(
		attribute.span(),
		"only structural attributes belong in an entry point",
	)
}

fn diagnostic(span: Span, message: &str) -> String {
	let start = span.start();
	format!("{}:{}: {message}", start.line, start.column + 1)
}
