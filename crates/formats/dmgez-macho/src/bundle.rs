//! Code-signing markers used to protect Finder metadata changes.

const PATH_SEPARATOR: char = '/';
const APP_BUNDLE_SUFFIX: [u8; 4] = *b".app";
const SIGNATURE_DIRECTORY: &str = "_CodeSignature";
const SIGNATURE_RESOURCES: &str = "CodeResources";
const BUNDLE_CONTENTS: &str = "Contents";
const FRAMEWORK_VERSIONS: &str = "Versions";

/// Known extended attribute names used by native code signing.
pub const SIGNING_ATTRIBUTE_NAMES: [&[u8]; 4] = [
	b"com.apple.cs.CodeDirectory",
	b"com.apple.cs.CodeRequirements",
	b"com.apple.cs.CodeRequirements-1",
	b"com.apple.cs.CodeSignature",
];

/// Tests a path component for the conventional application-bundle suffix.
#[must_use]
pub fn has_app_suffix(name: &[u8]) -> bool {
	name.last_chunk::<{ APP_BUNDLE_SUFFIX.len() }>()
		.is_some_and(|suffix| suffix.eq_ignore_ascii_case(&APP_BUNDLE_SUFFIX))
}

/// Tests an exact extended attribute name for native code-signing metadata.
#[must_use]
pub fn is_signing_attribute(name: &[u8]) -> bool {
	SIGNING_ATTRIBUTE_NAMES.contains(&name)
}

/// Infers a bundle root from a regular file's absolute, slash-separated path.
/// Recognizes direct, `Contents` and physical versioned-framework markers.
/// An empty root denotes a bundle at the volume root.
#[must_use]
pub fn signed_bundle_root(path: &str) -> Option<&str> {
	let (directory, resource) = path.rsplit_once(PATH_SEPARATOR)?;
	if !resource.eq_ignore_ascii_case(SIGNATURE_RESOURCES) {
		return None;
	}
	let (container, signature) = directory.rsplit_once(PATH_SEPARATOR)?;
	if !signature.eq_ignore_ascii_case(SIGNATURE_DIRECTORY) {
		return None;
	}
	let Some((parent, name)) = container.rsplit_once(PATH_SEPARATOR) else {
		return Some(container);
	};
	if name.eq_ignore_ascii_case(BUNDLE_CONTENTS) {
		return Some(parent);
	}
	if let Some((root, directory)) = parent.rsplit_once(PATH_SEPARATOR)
		&& directory.eq_ignore_ascii_case(FRAMEWORK_VERSIONS)
	{
		return Some(root);
	}
	Some(container)
}

/// Orders roots for [`inside_signed_bundle`] using ASCII case folding.
pub fn sort_signed_bundle_roots(roots: &mut [&str]) {
	roots.sort_unstable_by(|left, right| folded(left).cmp(folded(right)));
}

/// Checks a slash-separated path and its ancestors against sorted bundle roots.
/// Sort roots with [`sort_signed_bundle_roots`] before calling this function.
#[must_use]
pub fn inside_signed_bundle(path: &str, sorted_roots: &[&str]) -> bool {
	std::iter::once(path)
		.chain(
			path.rmatch_indices(PATH_SEPARATOR)
				.map(|(end, _)| &path[..end]),
		)
		.any(|component| {
			sorted_roots
				.binary_search_by(|root| folded(root).cmp(folded(component)))
				.is_ok()
		})
}

fn folded(path: &str) -> impl Iterator<Item = u8> + '_ {
	path.bytes().map(|byte| byte.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn matches_recognized_marker_roots_and_whole_ancestors() {
		let mut roots = [
			signed_bundle_root("/Zulu/Contents/_CodeSignature/CodeResources").unwrap(),
			signed_bundle_root("/alpha/Versions/A/_codesignature/coderesources").unwrap(),
			signed_bundle_root("/Middle/_CodeSignature/CodeResources").unwrap(),
		];
		assert_eq!(roots, ["/Zulu", "/alpha", "/Middle"]);
		sort_signed_bundle_roots(&mut roots);
		assert!(inside_signed_bundle("/Alpha/Versions/A/document", &roots));
		assert!(inside_signed_bundle("/Middle", &roots));
		assert!(!inside_signed_bundle("/MiddleSibling/document", &roots));
		assert_eq!(
			signed_bundle_root("/_CodeSignature/CodeResources"),
			Some("")
		);
		assert_eq!(signed_bundle_root("/Other/CodeResources"), None);
	}
}
