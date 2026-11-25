use dmgez_inspector::volume_queries::{SearchExtent, search_children};
use dmgez_vfs::volume::{NodeKind, RawName};

use super::{Counts, ROOT_CHILDREN, UNPAIRED_SURROGATE, view};

#[test]
fn reports_direct_child_names_without_looking_up_link_targets() {
	let counts = Counts::default();
	let mut view = view(&counts);
	let root = view.root().unwrap();
	let result = search_children(&mut view, &root, |name| {
		Ok(matches!(name, RawName::Utf16(_))
			|| matches!(name, RawName::Utf8(text) if text == "link"))
	})
	.unwrap();

	assert_eq!(result.extent, SearchExtent::Complete);
	assert_eq!(result.scanned_entries, ROOT_CHILDREN.len() as u64);
	assert_eq!(result.entries.len(), 2);
	assert_eq!(result.entries[0].kind, NodeKind::Symlink);
	assert_eq!(result.entries[0].name, RawName::Utf8("link".into()));
	assert_eq!(
		result.entries[1].name,
		RawName::Utf16(vec![UNPAIRED_SURROGATE, u16::from(b'x')]),
	);
	assert_eq!(counts.lookups.get(), 0);
	assert_eq!(counts.open_fork.get(), 0);
	assert_eq!(counts.link_reads.get(), 0);
}
