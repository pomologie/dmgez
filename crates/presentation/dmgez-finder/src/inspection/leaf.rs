use std::ops::Range;

use ds_parser::layout::{
	AllocatorOffsetTable, BTreeMaster, BlockId, BlockSize, BuddyOffset, EncodedAddress, Header,
	TableOfContents,
};

use super::ReadError;

const WORD_BYTES: usize = size_of::<u32>();
const MIN_TOC_ENTRY_BYTES: usize = size_of::<u8>() + WORD_BYTES;
const MIN_BLOCK_BYTES: usize = 1 << BlockSize::MINIMUM_LOG2;
const TREE_LEVEL: u32 = 0;
const NODE_COUNT: u32 = 1;

pub(super) struct Leaf<'a> {
	pub record_count: usize,
	pub records_offset: usize,
	pub records: &'a [u8],
}

pub(super) fn read(bytes: &[u8]) -> Result<Leaf<'_>, ReadError> {
	let allocator = allocator_range(bytes)?;
	let mut bookkeeping = Cursor::new(&bytes[allocator.clone()]);
	let addresses = bookkeeping.count()?;
	bookkeeping.word()?;
	let address_bytes = addresses
		.checked_mul(WORD_BYTES)
		.ok_or(ReadError::Allocator)?;
	let table = bookkeeping.take(address_bytes)?;
	let toc_offset = AllocatorOffsetTable::toc_offset(addresses);
	let toc = bytes[allocator.clone()]
		.get(toc_offset..)
		.ok_or(ReadError::Allocator)?;

	// Index zero names the bookkeeping block itself. Read just the two
	// blocks reached from DSDB, without following any free-list links.
	if table_range(bytes, table, 0)? != allocator {
		return Err(ReadError::Allocator);
	}
	let master_id = master_id(toc)?;
	let master = table_range(bytes, table, master_id)?;
	if overlaps(&master, &allocator) {
		return Err(ReadError::Allocator);
	}
	let mut tree = Cursor::new(&bytes[master.clone()]);
	let root_id = tree.count()?;
	let depth = tree.word()?;
	let record_count = tree.count()?;
	let nodes = tree.word()?;
	let page_bytes = tree.word()?;
	if depth != TREE_LEVEL || nodes != NODE_COUNT || page_bytes != BTreeMaster::PAGE_SIZE.bytes() {
		return Err(ReadError::UnsupportedTree);
	}

	let root = table_range(bytes, table, root_id)?;
	if overlaps(&root, &master) || overlaps(&root, &allocator) {
		return Err(ReadError::Allocator);
	}
	let root_offset = root.start;
	let root = &bytes[root];
	let logical_bytes = root.len().min(BTreeMaster::PAGE_SIZE.as_usize());
	let mut leaf = Cursor::new(&root[..logical_bytes]);
	if leaf.word()? != BlockId::NULL.get() {
		return Err(ReadError::UnsupportedTree);
	}
	if leaf.count()? != record_count {
		return Err(ReadError::RecordCount);
	}
	Ok(Leaf {
		record_count,
		records_offset: root_offset + logical_bytes - leaf.remaining().len(),
		records: leaf.remaining(),
	})
}

fn allocator_range(bytes: &[u8]) -> Result<Range<usize>, ReadError> {
	let mut header = Cursor::new(bytes.get(..Header::LEN).ok_or(ReadError::Header)?);
	if header.word()? != Header::ALIGNMENT || header.array()? != Header::MAGIC.bytes() {
		return Err(ReadError::Header);
	}
	let offset = header.word()?;
	let length = header.count()?;
	if offset != header.word()? {
		return Err(ReadError::Header);
	}
	block_range(bytes, BuddyOffset::new(offset), length)
}

fn table_range(bytes: &[u8], table: &[u8], id: usize) -> Result<Range<usize>, ReadError> {
	let start = id.checked_mul(WORD_BYTES).ok_or(ReadError::Allocator)?;
	let mut entry = Cursor::new(table.get(start..).ok_or(ReadError::Allocator)?);
	let address = EncodedAddress::new(entry.word().map_err(|_| ReadError::Allocator)?);
	if address == EncodedAddress::EMPTY {
		return Err(ReadError::Allocator);
	}
	let size = address.block_size().map_err(|_| ReadError::Allocator)?;
	block_range(bytes, address.buddy_offset(), size.as_usize())
}

fn block_range(
	bytes: &[u8],
	offset: BuddyOffset,
	length: usize,
) -> Result<Range<usize>, ReadError> {
	let from = offset.file_index().map_err(|_| ReadError::Allocator)?;
	let to = from.checked_add(length).ok_or(ReadError::Allocator)?;
	if from < Header::LEN
		|| to > bytes.len()
		|| length < MIN_BLOCK_BYTES
		|| !length.is_power_of_two()
		|| !(from - BuddyOffset::FILE_CONTENT_OFFSET).is_multiple_of(length)
	{
		return Err(ReadError::Allocator);
	}
	Ok(from..to)
}

fn master_id(toc: &[u8]) -> Result<usize, ReadError> {
	let mut toc = Cursor::new(toc);
	let entries = toc.count()?;
	if entries > toc.remaining().len() / MIN_TOC_ENTRY_BYTES {
		return Err(ReadError::Allocator);
	}
	let mut master = None;
	for _ in 0..entries {
		let [name_len] = toc.array()?;
		let name = toc.take(usize::from(name_len))?;
		let id = toc.count()?;
		if name == TableOfContents::DSDB.as_bytes() && master.replace(id).is_some() {
			return Err(ReadError::Allocator);
		}
	}
	master.ok_or(ReadError::MissingTree)
}

fn overlaps(left: &Range<usize>, right: &Range<usize>) -> bool {
	left.start < right.end && right.start < left.end
}

pub(super) struct Cursor<'a> {
	remaining: &'a [u8],
}

impl<'a> Cursor<'a> {
	pub(super) const fn new(remaining: &'a [u8]) -> Self {
		Self { remaining }
	}

	pub(super) const fn remaining(&self) -> &'a [u8] {
		self.remaining
	}

	pub(super) fn take(&mut self, length: usize) -> Result<&'a [u8], ReadError> {
		let (bytes, rest) = self
			.remaining
			.split_at_checked(length)
			.ok_or(ReadError::Truncated)?;
		self.remaining = rest;
		Ok(bytes)
	}

	pub(super) fn array<const N: usize>(&mut self) -> Result<[u8; N], ReadError> {
		self.take(N)?.try_into().map_err(|_| ReadError::Truncated)
	}

	pub(super) fn word(&mut self) -> Result<u32, ReadError> {
		Ok(u32::from_be_bytes(self.array()?))
	}

	pub(super) fn count(&mut self) -> Result<usize, ReadError> {
		self.word()?.try_into().map_err(|_| ReadError::Truncated)
	}

	pub(super) fn counted_bytes(&mut self) -> Result<&'a [u8], ReadError> {
		let length = self.count()?;
		self.take(length)
	}
}
