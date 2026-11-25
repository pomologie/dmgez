use crate::CliResult;

#[cfg(any(feature = "image-edit", feature = "project-edit"))]
pub(crate) use dmgez_runtime::project_input::read_limited;

pub(crate) fn definitions(raw: &[String]) -> CliResult<std::collections::BTreeMap<String, String>> {
	const DEFINITION_SEPARATOR: char = '=';
	const INVALID_DEFINITION: &str = "expected a definition in NAME=VALUE form";
	let mut defines = std::collections::BTreeMap::new();
	for definition in raw {
		let (name, value) = definition
			.split_once(DEFINITION_SEPARATOR)
			.ok_or(INVALID_DEFINITION)?;
		defines.insert(name.to_owned(), value.to_owned());
	}
	Ok(defines)
}
