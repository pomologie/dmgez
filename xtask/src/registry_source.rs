//! Rewrite local crate paths while retaining the original source bytes around edits.

use std::{
	collections::{BTreeMap, BTreeSet},
	error::Error,
	fs,
	ops::Range,
	path::PathBuf,
};

use clap::Args;
use proc_macro2::{Span, TokenStream, TokenTree};
use syn::{
	Attribute, Expr, ExprLit, Ident, ItemUse, Lit, LitStr, Macro, Meta, Path, Token, UseTree,
	punctuated::Punctuated,
	visit::{self, Visit},
};

#[cfg(test)]
mod tests;

#[derive(Args)]
pub(crate) struct Arguments {
	#[arg(long)]
	input: PathBuf,
	#[arg(long)]
	output: PathBuf,
	/// Private module that replaces the original crate root.
	#[arg(long, value_parser = parse_identifier)]
	module: Option<String>,
	/// Original dependency alias and private module, as ALIAS=MODULE.
	#[arg(long, value_parser = parse_dependency)]
	dependency: Vec<(String, String)>,
	/// A Cargo feature enabled in the qualified source profile.
	#[arg(long)]
	active_feature: Vec<String>,
	/// Drop a crate-level no_std attribute when hosting a portable engine.
	#[arg(long)]
	strip_no_std: bool,
}

fn parse_identifier(value: &str) -> Result<String, String> {
	syn::parse_str::<Ident>(value)
		.map(|ident| ident.to_string())
		.map_err(|error| error.to_string())
}

fn parse_dependency(assignment: &str) -> Result<(String, String), String> {
	let (alias, module) = assignment.split_once('=').ok_or("expected ALIAS=MODULE")?;
	Ok((parse_identifier(alias)?, parse_identifier(module)?))
}

pub(crate) fn rewrite(arguments: Arguments) -> Result<(), Box<dyn Error>> {
	let source = fs::read_to_string(&arguments.input)?;
	let output = rewrite_source(&source, &arguments)
		.map_err(|error| format!("{}: {error}", arguments.input.display()))?;
	fs::write(arguments.output, output)?;
	Ok(())
}

fn rewrite_source(source: &str, arguments: &Arguments) -> Result<String, Box<dyn Error>> {
	let file = syn::parse_file(source)?;
	let mut transform = Transformer {
		module: arguments.module.as_deref(),
		dependencies: BTreeMap::new(),
		active_features: arguments
			.active_feature
			.iter()
			.map(String::as_str)
			.collect(),
		edits: BTreeMap::new(),
	};
	for (alias, module) in &arguments.dependency {
		if transform
			.dependencies
			.insert(alias.as_str(), module.as_str())
			.is_some()
		{
			return Err(format!("duplicate dependency alias {alias}").into());
		}
	}
	for attribute in &file.attrs {
		if attribute.path().is_ident("no_std") && transform.module.is_some() {
			if !arguments.strip_no_std {
				return Err("moving a no_std crate into a module requires --strip-no-std".into());
			}
			let start = attribute.pound_token.span.byte_range().start;
			let end = attribute.bracket_token.span.close().byte_range().end;
			transform.replace(start..end, String::new());
		}
	}
	transform.visit_file(&file);
	let mut output = String::with_capacity(source.len());
	let mut position = 0;
	for (start, (end, replacement)) in transform.edits {
		if start < position || !source.is_char_boundary(start) || !source.is_char_boundary(end) {
			return Err("invalid or overlapping Rust source edit spans".into());
		}
		output.push_str(&source[position..start]);
		output.push_str(&replacement);
		position = end;
	}
	output.push_str(&source[position..]);
	Ok(output)
}

struct Transformer<'a> {
	module: Option<&'a str>,
	dependencies: BTreeMap<&'a str, &'a str>,
	active_features: BTreeSet<&'a str>,
	edits: BTreeMap<usize, (usize, String)>,
}

impl Transformer<'_> {
	fn replace(&mut self, range: Range<usize>, replacement: String) {
		self.edits.insert(range.start, (range.end, replacement));
	}

	fn private_module(&self, ident: &Ident) -> Option<&str> {
		let name = ident.to_string();
		if name == "crate" {
			self.module
		} else {
			self.dependencies.get(name.as_str()).copied()
		}
	}

	fn replace_root(&mut self, ident: &Ident, absolute: Option<Span>) {
		if let Some(module) = self.private_module(ident) {
			let range = ident.span().byte_range();
			let start = absolute.map_or(range.start, |span| span.byte_range().start);
			self.replace(start..range.end, format!("crate::internal::{module}"));
		}
	}

	fn rewrite_serde_paths(&mut self, attribute: &Attribute) {
		let Ok(arguments) =
			attribute.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
		else {
			return;
		};
		for argument in arguments {
			let Meta::NameValue(option) = argument else {
				continue;
			};
			if ![
				"serialize_with",
				"deserialize_with",
				"with",
				"default",
				"crate",
			]
			.iter()
			.any(|name| option.path.is_ident(name))
			{
				continue;
			}
			let Expr::Lit(ExprLit {
				lit: Lit::Str(value),
				..
			}) = option.value
			else {
				continue;
			};
			let Ok(path) = value.parse::<Path>() else {
				continue;
			};
			let Some(module) = path
				.segments
				.first()
				.and_then(|segment| self.private_module(&segment.ident))
			else {
				continue;
			};
			let module = Ident::new(module, Span::call_site());
			let suffix = path.segments.iter().skip(1);
			let replacement = quote::quote!(crate::internal::#module #(::#suffix)*);
			let replacement = LitStr::new(&replacement.to_string(), Span::call_site());
			self.replace(
				value.span().byte_range(),
				quote::quote!(#replacement).to_string(),
			);
		}
	}

	fn replace_use(&mut self, tree: &UseTree, absolute: Option<Span>) {
		match tree {
			UseTree::Path(path) => self.replace_root(&path.ident, absolute),
			UseTree::Name(name) => self.replace_root(&name.ident, absolute),
			UseTree::Rename(rename) => self.replace_root(&rename.ident, absolute),
			UseTree::Group(group) => {
				for item in &group.items {
					self.replace_use(item, None);
				}
			}
			UseTree::Glob(_) => {}
		}
	}

	fn rewrite_tokens(&mut self, stream: TokenStream, cfg: bool) {
		let tokens: Vec<_> = stream.into_iter().collect();
		for (index, token) in tokens.iter().enumerate() {
			match token {
				TokenTree::Group(group) => self.rewrite_tokens(group.stream(), cfg),
				TokenTree::Ident(ident) if cfg && ident == "feature" => {
					if let [TokenTree::Punct(eq), TokenTree::Literal(literal), ..] =
						&tokens[index + 1..]
						&& eq.as_char() == '='
						&& let Ok(feature) = syn::parse_str::<LitStr>(&literal.to_string())
					{
						let condition = if self.active_features.contains(feature.value().as_str()) {
							"all()"
						} else {
							"any()"
						};
						self.replace(
							ident.span().byte_range().start..literal.span().byte_range().end,
							condition.to_owned(),
						);
					}
				}
				TokenTree::Ident(ident) if separators(&tokens[index + 1..]) => {
					let absolute = index
						.checked_sub(2)
						.filter(|&start| separators(&tokens[start..index]))
						.map(|start| tokens[start].span());
					if absolute.is_none()
						|| index == 2
						|| !matches!(tokens[index - 3], TokenTree::Ident(_))
					{
						self.replace_root(ident, absolute);
					}
				}
				_ => {}
			}
		}
	}
}

fn separators(tokens: &[TokenTree]) -> bool {
	matches!(tokens, [TokenTree::Punct(first), TokenTree::Punct(second), ..]
		if first.as_char() == ':' && second.as_char() == ':')
}

impl<'ast> Visit<'ast> for Transformer<'_> {
	fn visit_path(&mut self, path: &'ast Path) {
		if path.segments.len() > 1
			&& let Some(first) = path.segments.first()
		{
			self.replace_root(
				&first.ident,
				path.leading_colon.as_ref().map(|colon| colon.spans[0]),
			);
		}
		visit::visit_path(self, path);
	}

	fn visit_item_use(&mut self, item: &'ast ItemUse) {
		self.replace_use(
			&item.tree,
			item.leading_colon.as_ref().map(|colon| colon.spans[0]),
		);
		visit::visit_item_use(self, item);
	}

	fn visit_attribute(&mut self, attribute: &'ast Attribute) {
		if attribute.path().is_ident("serde") {
			self.rewrite_serde_paths(attribute);
		}
		if let Meta::List(list) = &attribute.meta {
			let cfg = list.path.is_ident("cfg") || list.path.is_ident("cfg_attr");
			self.rewrite_tokens(list.tokens.clone(), cfg);
		}
		visit::visit_attribute(self, attribute);
	}

	fn visit_macro(&mut self, invocation: &'ast Macro) {
		self.rewrite_tokens(invocation.tokens.clone(), invocation.path.is_ident("cfg"));
		visit::visit_macro(self, invocation);
	}
}
