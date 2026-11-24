use std::{
	ffi::OsStr,
	mem::size_of,
	path::{Path, PathBuf},
};

use dmgez_config::{CopySource, LocalPath, Text, VirtualPath};
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use rustix::{
	fd::AsFd,
	fs::{Dir, FileType},
};

use super::{
	Body, Budget, Catalog, EntryKind, Error, FinderOverrides, Origin, PATH_SEPARATOR, PatternList,
	SourceSnapshot,
	host::{self, Anchor, OutputProbe},
};

const MAX_PATTERNS: usize = 1024;
const MAX_PATTERN_BYTES: usize = 64 << 10;

pub(super) struct Walker<'a> {
	config_dir: &'a Path,
	output: PathBuf,
	host: Option<Host>,
	visited: u64,
}

struct Host {
	anchor: Anchor,
	output: OutputProbe,
}

impl<'a> Walker<'a> {
	pub(super) fn new(config_dir: &'a Path, output: &LocalPath) -> Self {
		Self {
			config_dir,
			output: output.resolve(config_dir),
			host: None,
			visited: 0,
		}
	}

	pub(super) fn copy(
		&mut self,
		destination: &VirtualPath,
		copy: &CopySource,
		finder: FinderOverrides,
		catalog: &mut Catalog,
		cancelled: &mut impl FnMut() -> bool,
	) -> Result<(), Error> {
		record_visit(&mut self.visited, catalog)?;
		let filters = Filters::new(copy)?;
		let host = match &mut self.host {
			Some(host) => host,
			None => {
				let anchor = Anchor::new()?;
				let output = anchor.probe_output(&self.output)?;
				self.host.insert(Host { anchor, output })
			}
		};
		let path = copy.source.resolve(self.config_dir);
		let leaf = host.anchor.locate(&path)?;
		let source = host
			.anchor
			.snapshot(&leaf.directory, leaf.name, path.clone())?;
		host.output.check(&source)?;

		if source.kind != FileType::Directory || !copy.recursive {
			if !filters.is_empty() {
				return Err(Error::PatternsRequireDirectory { path });
			}
			let body = capture(&leaf.directory, leaf.name, source, finder)?;
			return catalog.admit(destination.clone(), body);
		}

		let directory = host::open_directory(&leaf.directory, leaf.name, &source)?;
		let root = Frame {
			directory,
			source,
			relative: PathBuf::new(),
			included: true,
			finder,
		};
		walk_tree(
			host,
			destination,
			&filters,
			root,
			catalog,
			&mut self.visited,
			cancelled,
		)
	}
}

struct Frame {
	directory: Dir,
	source: SourceSnapshot,
	relative: PathBuf,
	included: bool,
	finder: FinderOverrides,
}

impl Frame {
	fn bytes(&self) -> Result<u64, Error> {
		let bytes = size_of::<Frame>()
			.checked_add(self.relative.as_os_str().len())
			.and_then(|bytes| bytes.checked_add(self.source.path.as_os_str().len()))
			.ok_or(Error::Limit(Budget::Metadata))?;
		u64::try_from(bytes).map_err(|_| Error::Limit(Budget::Metadata))
	}

	fn into_body(self) -> Body {
		Body {
			kind: EntryKind::Directory,
			source: Some(self.source),
			finder: self.finder,
			origin: Origin::Host,
		}
	}
}

fn walk_tree(
	host: &Host,
	destination: &VirtualPath,
	filters: &Filters,
	root: Frame,
	catalog: &mut Catalog,
	visited: &mut u64,
	cancelled: &mut impl FnMut() -> bool,
) -> Result<(), Error> {
	let mut stack = Vec::new();
	let mut scratch_bytes = 0_u64;
	push_frame(&mut stack, &mut scratch_bytes, root, catalog)?;
	while let Some(frame) = stack.last_mut() {
		if cancelled() {
			return Err(Error::Cancelled);
		}
		let Some(name) = host::next_name(&mut frame.directory, &frame.source)? else {
			let directory = host::directory_handle(&frame.directory, &frame.source)?;
			host::match_opened(&frame.source, directory)?;
			frame.source.validate()?;
			let Some(frame) = stack.pop() else { break };
			scratch_bytes = scratch_bytes
				.checked_sub(frame.bytes()?)
				.ok_or(Error::Limit(Budget::Metadata))?;
			if frame.included {
				let path = child_destination(destination, &frame.relative)?;
				catalog.admit(path, frame.into_body())?;
				check_combined_metadata(scratch_bytes, catalog)?;
				include_parent(&mut stack);
			}
			continue;
		};
		record_visit(visited, catalog)?;
		let relative = frame.relative.join(&name);
		if filters.exclude.is_match(&relative) {
			continue;
		}
		let next_depth = destination
			.components()
			.count()
			.checked_add(relative.components().count())
			.ok_or(Error::Limit(Budget::Depth))?;
		if next_depth > catalog.limits.max_depth {
			return Err(Error::Limit(Budget::Depth));
		}
		let directory = host::directory_handle(&frame.directory, &frame.source)?;
		let source = host
			.anchor
			.snapshot(directory, &name, frame.source.path.join(&name))?;
		let included = filters.include.is_empty() || filters.include.is_match(&relative);
		if source.kind == FileType::Directory {
			host.output.check(&source)?;
			let child = host::open_directory(directory, &name, &source)?;
			let frame = Frame {
				directory: child,
				source,
				relative,
				included,
				finder: FinderOverrides::default(),
			};
			push_frame(&mut stack, &mut scratch_bytes, frame, catalog)?;
		} else if included {
			host.output.check(&source)?;
			let path = child_destination(destination, &relative)?;
			let body = capture(directory, &name, source, FinderOverrides::default())?;
			catalog.admit(path, body)?;
			check_combined_metadata(scratch_bytes, catalog)?;
			frame.included = true;
		}
	}
	Ok(())
}

fn include_parent(stack: &mut [Frame]) {
	if let Some(parent) = stack.last_mut() {
		parent.included = true;
	}
}

fn push_frame(
	stack: &mut Vec<Frame>,
	scratch_bytes: &mut u64,
	frame: Frame,
	catalog: &Catalog,
) -> Result<(), Error> {
	*scratch_bytes = scratch_bytes
		.checked_add(frame.bytes()?)
		.ok_or(Error::Limit(Budget::Metadata))?;
	check_combined_metadata(*scratch_bytes, catalog)?;
	stack.try_reserve(1)?;
	stack.push(frame);
	Ok(())
}

fn check_combined_metadata(scratch_bytes: u64, catalog: &Catalog) -> Result<(), Error> {
	catalog
		.metadata_bytes()
		.checked_add(scratch_bytes)
		.filter(|&bytes| bytes <= catalog.limits.max_metadata_bytes)
		.map(|_| ())
		.ok_or(Error::Limit(Budget::Metadata))
}

fn record_visit(count: &mut u64, catalog: &Catalog) -> Result<(), Error> {
	*count = count
		.checked_add(1)
		.filter(|&count| count <= catalog.limits.max_visited)
		.ok_or(Error::Limit(Budget::Visited))?;
	Ok(())
}

fn capture(
	parent: impl AsFd,
	name: &OsStr,
	source: SourceSnapshot,
	finder: FinderOverrides,
) -> Result<Body, Error> {
	let kind = match source.kind {
		FileType::Directory => EntryKind::Directory,
		FileType::RegularFile => EntryKind::File,
		FileType::Symlink => EntryKind::Symlink(host::read_link(parent, name, &source)?),
		_ => return Err(Error::UnsupportedSource { path: source.path }),
	};
	Ok(Body {
		kind,
		source: Some(source),
		finder,
		origin: Origin::Host,
	})
}

fn child_destination(root: &VirtualPath, relative: &Path) -> Result<VirtualPath, Error> {
	if relative.as_os_str().is_empty() {
		return Ok(root.clone());
	}
	let path_error = || Error::InvalidSourceName {
		path: relative.into(),
	};
	let relative = relative.to_str().ok_or_else(path_error)?;
	format!("{root}{PATH_SEPARATOR}{relative}")
		.parse()
		.map_err(|_| path_error())
}

struct Filters {
	include: GlobSet,
	exclude: GlobSet,
}

impl Filters {
	fn new(copy: &CopySource) -> Result<Self, Error> {
		let mut bytes = copy.include.iter().chain(&copy.exclude);
		let count = copy.include.len().saturating_add(copy.exclude.len());
		if count > MAX_PATTERNS {
			return Err(Error::Limit(Budget::Patterns));
		}
		bytes
			.try_fold(0_usize, |sum, pattern| {
				sum.checked_add(pattern.as_str().len())
			})
			.filter(|&bytes| bytes <= MAX_PATTERN_BYTES)
			.ok_or(Error::Limit(Budget::Patterns))?;
		Ok(Self {
			include: glob_set(&copy.include, PatternList::Include)?,
			exclude: glob_set(&copy.exclude, PatternList::Exclude)?,
		})
	}

	fn is_empty(&self) -> bool {
		self.include.is_empty() && self.exclude.is_empty()
	}
}

fn glob_set(patterns: &[Text], list: PatternList) -> Result<GlobSet, Error> {
	let mut builder = GlobSetBuilder::new();
	for (index, pattern) in patterns.iter().enumerate() {
		let glob = GlobBuilder::new(pattern.as_str())
			.literal_separator(true)
			.backslash_escape(false)
			.build()
			.map_err(|source| Error::InvalidPattern {
				list,
				index,
				source,
			})?;
		builder.add(glob);
	}
	builder
		.build()
		.map_err(|source| Error::PatternSet { list, source })
}
