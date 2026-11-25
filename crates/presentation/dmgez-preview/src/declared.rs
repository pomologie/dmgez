use std::collections::{BTreeMap, btree_map::Entry};

use dmgez_config::{Content, Coordinate, Payload, Project, VirtualPath, VirtualPathError};
use thiserror::Error;

use crate::{ItemInput, LayoutError, ProjectFolder};

const IMAGE_ROOT: &str = "/";
const PATH_SEPARATOR: char = '/';
const MAX_ENTRIES: usize = 16_384;
const MAX_PATH_BYTES: usize = 4_096;

/// Failure to index or lay out the declared contents of a project.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum DeclaredTreeError {
	#[error("the declared project tree exceeds the preview limit of {MAX_ENTRIES} entries")]
	EntryLimit,
	#[error("a virtual path exceeds the preview limit of {MAX_PATH_BYTES} bytes")]
	PathLimit,
	#[error("a declaration cannot replace the root of the preview tree")]
	RootDestination,
	#[error("the declared project tree contains a duplicate destination {0:?}")]
	DuplicateDestination(String),
	#[error("the project contains an invalid preview path")]
	Path(#[from] VirtualPathError),
	#[error(transparent)]
	Layout(#[from] LayoutError),
}

/// A project declaration or an implied directory in the design tree.
#[derive(Debug)]
pub struct DeclaredEntry {
	path: VirtualPath,
	directory: bool,
	position: Option<[Coordinate; 2]>,
	hidden: bool,
	hide_extension: bool,
}

impl DeclaredEntry {
	/// Returns the path within the image.
	#[must_use]
	pub fn path(&self) -> &VirtualPath {
		&self.path
	}

	/// Returns whether this is an explicit or implied directory.
	#[must_use]
	pub const fn is_directory(&self) -> bool {
		self.directory
	}
}

/// An owned snapshot of the typed project declarations and implied directories.
/// Building the design tree performs no host access or recursive source expansion.
#[derive(Debug)]
pub struct DeclaredTree {
	folders: BTreeMap<String, Vec<DeclaredEntry>>,
}

impl DeclaredTree {
	/// Indexes content declarations and their ancestors at bounded size.
	///
	/// # Errors
	/// Returns an error if a path or tree exceeds the preview limits, or if
	/// a declaration duplicates an existing entry or replaces the root.
	pub fn from_project(project: &Project) -> Result<Self, DeclaredTreeError> {
		let mut nodes: BTreeMap<&str, Option<&Content>> = BTreeMap::new();
		for content in &project.contents {
			let path = content.destination.as_str();
			if path.len() > MAX_PATH_BYTES {
				return Err(DeclaredTreeError::PathLimit);
			}
			if content.destination.is_root() {
				return Err(DeclaredTreeError::RootDestination);
			}
			if matches!(nodes.insert(path, Some(content)), Some(Some(_))) {
				return Err(DeclaredTreeError::DuplicateDestination(path.to_owned()));
			}
			check_entry_limit(nodes.len())?;
			let mut ancestor = parent(path);
			while ancestor != IMAGE_ROOT {
				let Entry::Vacant(slot) = nodes.entry(ancestor) else {
					// A previous declaration already indexed the rest of this chain.
					break;
				};
				slot.insert(None);
				check_entry_limit(nodes.len())?;
				ancestor = parent(ancestor);
			}
		}
		let mut folders = BTreeMap::<&str, Vec<DeclaredEntry>>::new();
		for (path, declaration) in nodes {
			let entry = if let Some(content) = declaration {
				DeclaredEntry {
					path: content.destination.clone(),
					directory: matches!(content.payload, Payload::Directory),
					position: content.position,
					hidden: content.hidden.unwrap_or_default(),
					hide_extension: content.hide_extension.unwrap_or_default(),
				}
			} else {
				DeclaredEntry {
					path: path.parse()?,
					directory: true,
					position: None,
					hidden: false,
					hide_extension: false,
				}
			};
			folders.entry(parent(path)).or_default().push(entry);
		}
		Ok(Self {
			folders: folders
				.into_iter()
				.map(|(parent, entries)| (parent.to_owned(), entries))
				.collect(),
		})
	}

	/// Returns whether this path identifies the root or a directory in the design tree.
	#[must_use]
	pub fn contains_folder(&self, folder: &VirtualPath) -> bool {
		folder.is_root()
			|| self
				.folders
				.get(parent(folder.as_str()))
				.is_some_and(|siblings| {
					siblings
						.binary_search_by(|entry| entry.path.cmp(folder))
						.is_ok_and(|index| siblings[index].is_directory())
				})
	}

	/// Returns the immediate declarations and implied directories in lexical path order.
	#[must_use]
	pub fn children(&self, folder: &VirtualPath) -> &[DeclaredEntry] {
		self.folders.get(folder.as_str()).map_or(&[], Vec::as_slice)
	}

	/// Combines this snapshot's children with the current project Finder settings.
	/// Call [`Self::contains_folder`] if the caller requires an existing directory.
	///
	/// # Errors
	/// Propagates invalid or unsupported Finder layout settings.
	pub fn layout<'layout>(
		&'layout self,
		project: &'layout Project,
		folder: &VirtualPath,
	) -> Result<ProjectFolder<'layout>, DeclaredTreeError> {
		let inputs: Vec<_> = self
			.children(folder)
			.iter()
			.map(|entry| ItemInput {
				destination: &entry.path,
				position: entry.position,
				hidden: entry.hidden,
				hide_extension: entry.hide_extension,
			})
			.collect();
		Ok(ProjectFolder::from_project(project, folder, &inputs)?)
	}
}

fn parent(path: &str) -> &str {
	path.rsplit_once(PATH_SEPARATOR)
		.and_then(|(parent, _)| (!parent.is_empty()).then_some(parent))
		.unwrap_or(IMAGE_ROOT)
}

fn check_entry_limit(entries: usize) -> Result<(), DeclaredTreeError> {
	if entries > MAX_ENTRIES {
		return Err(DeclaredTreeError::EntryLimit);
	}
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{FolderPoint, PositionSource};

	const IMAGE: &str = "schema_version = 1\n[image]\nname = 'Declared'\noutput = 'preview.dmg'\n";
	const CONTENTS: &str = r#"
[[contents]]
source = 'missing-on-purpose/Recursive'
destination = '/Docs/Guides/Manual.txt'
position = [40, 60]
hidden = true
hide_extension = true

[[contents]]
type = 'directory'
destination = '/Docs'

[[contents]]
type = 'directory'
destination = '/Empty'
"#;

	fn sample() -> Project {
		Project::parse(&format!("{IMAGE}{CONTENTS}")).unwrap()
	}

	#[test]
	fn resolves_explicit_and_implied_folders_from_declared_contents() {
		let project = sample();
		let tree = DeclaredTree::from_project(&project).unwrap();
		let root = IMAGE_ROOT.parse().unwrap();
		let guides = "/Docs/Guides".parse().unwrap();
		for folder in [
			&root,
			&guides,
			&"/Docs".parse().unwrap(),
			&"/Empty".parse().unwrap(),
		] {
			assert!(tree.contains_folder(folder));
		}
		assert!(!tree.contains_folder(&"/docs".parse().unwrap()));
		assert!(!tree.contains_folder(&"/Docs/Guides/Manual.txt".parse().unwrap()));
		assert_eq!(
			tree.children(&root)
				.iter()
				.map(|entry| entry.path().as_str())
				.collect::<Vec<_>>(),
			["/Docs", "/Empty"]
		);
		let layout = tree.layout(&project, &guides).unwrap();
		let [manual] = layout.items() else {
			panic!("expected one declaration")
		};
		assert_eq!(manual.center, FolderPoint::new(40.0, 60.0).unwrap());
		assert_eq!(manual.position_source, PositionSource::Content);
		assert!(manual.hidden && manual.hide_extension);
	}

	#[test]
	fn charges_implied_entries_and_path_bytes_to_preview_limits() {
		let mut project = sample();
		let mut content = project.contents.remove(0);
		let multibyte = "é";
		let name = multibyte.repeat((MAX_PATH_BYTES - IMAGE_ROOT.len()) / multibyte.len());
		content.destination = format!("/{name}x").parse().unwrap();
		project.contents = vec![content.clone()];
		assert!(DeclaredTree::from_project(&project).is_ok());
		project.contents[0].destination = format!("/{name}{multibyte}").parse().unwrap();
		assert!(matches!(
			DeclaredTree::from_project(&project),
			Err(DeclaredTreeError::PathLimit)
		));

		project.contents = (0..MAX_ENTRIES / 2)
			.map(|index| {
				let mut entry = content.clone();
				entry.destination = format!("/{index}/Item").parse().unwrap();
				entry
			})
			.collect();
		assert!(DeclaredTree::from_project(&project).is_ok());
		project.contents.push(content);
		assert!(matches!(
			DeclaredTree::from_project(&project),
			Err(DeclaredTreeError::EntryLimit)
		));
	}
}
