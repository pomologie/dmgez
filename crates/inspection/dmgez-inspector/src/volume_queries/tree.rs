use std::{collections::VecDeque, ops::Range};

use dmgez_vfs::volume::{NodeHandle, NodeKind, VolumeBackend, VolumeView};

use crate::{ReportOperation, SCHEMA_VERSION, SelectedWindowId};

use super::{Action, Error, Name, TreeEntryReport, TreeReport, next_page, resolve};

/// Default maximum number of edges below the starting directory.
pub const DEFAULT_TREE_DEPTH: u16 = 8;
/// Maximum supported query depth.
pub const MAX_TREE_DEPTH: u16 = 64;
/// Default entry ceiling for a tree report.
pub const DEFAULT_TREE_LIMIT: u32 = 1024;
/// Maximum number of descendants a tree report can retain.
pub const MAX_TREE_LIMIT: u32 = 10_000;

#[derive(Default)]
struct Row {
	entry: Option<TreeEntryReport>,
	children: Range<usize>,
}

struct PendingDirectory<Id> {
	index: usize,
	node: NodeHandle<Id>,
	depth: u16,
}

/// Reads a bounded tree of directory entries without following symbolic links.
///
/// The starting directory is given by `path`. Its children start at depth one.
/// The report admits entries breadth-first under the entry limit and presents
/// them parent-first in the engine's native sibling order. The requested depth
/// limits the query scope independently of entry truncation.
///
/// # Errors
/// Rejects invalid paths, non-directories, invalid limits, and failed directory reads.
pub fn tree<B: VolumeBackend>(
	view: &mut VolumeView<B>,
	path: &str,
	depth: u16,
	limit: u32,
) -> Result<TreeReport, Error<B::Error>> {
	if depth > MAX_TREE_DEPTH {
		return Err(Error::TreeDepth {
			maximum: MAX_TREE_DEPTH,
		});
	}
	if limit == 0 || limit > MAX_TREE_LIMIT {
		return Err(Error::TreeLimit {
			maximum: MAX_TREE_LIMIT,
		});
	}
	let volume = SelectedWindowId::from_volume(view.partition()).ok_or(Error::UnsupportedVolume)?;
	let root = resolve(view, path)?;
	root.require_directory()?;
	let mut report = TreeReport {
		schema_version: SCHEMA_VERSION,
		operation: ReportOperation::Tree,
		volume,
		path: path.into(),
		entries: Vec::new(),
		truncated: false,
	};
	if depth == 0 {
		return Ok(report);
	}

	let row_allowance = limit as usize + 1;
	let mut rows = Vec::new();
	rows.try_reserve_exact(row_allowance)?;
	rows.push(Row::default());
	let mut pending = VecDeque::new();
	pending.try_reserve(1)?;
	pending.push_back(PendingDirectory {
		index: 0,
		node: root.node,
		depth: 0,
	});
	while let Some(current) = pending.pop_front() {
		let child_depth = current.depth + 1;
		let mut cursor = view
			.read_dir(&current.node)
			.map_err(|error| Error::view(Action::Directory, error))?;
		let first_child = rows.len();
		'pages: loop {
			let page = next_page(&mut cursor, (row_allowance - rows.len() + 1) as u64)?;
			for child in page.entries {
				if rows.len() == row_allowance {
					report.truncated = true;
					break 'pages;
				}
				let name = Name::from_native(child.name).ok_or(Error::UnsupportedName)?;
				let index = rows.len();
				rows.push(Row {
					entry: Some(TreeEntryReport {
						depth: child_depth,
						name,
						kind: child.kind.into(),
					}),
					children: 0..0,
				});
				if child.kind == NodeKind::Directory && child_depth < depth {
					pending.try_reserve(1)?;
					pending.push_back(PendingDirectory {
						index,
						node: child.node,
						depth: child_depth,
					});
				}
			}
			if page.complete {
				break;
			}
		}
		rows[current.index].children = first_child..rows.len();
		if report.truncated {
			break;
		}
	}
	report.entries.try_reserve_exact(rows.len() - 1)?;
	write_rows(0, &mut rows, &mut report.entries);
	Ok(report)
}

fn write_rows(index: usize, rows: &mut [Row], output: &mut Vec<TreeEntryReport>) {
	let row = std::mem::take(&mut rows[index]);
	if let Some(entry) = row.entry {
		output.push(entry);
	}
	for child in row.children {
		write_rows(child, rows, output);
	}
}
