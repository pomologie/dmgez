//! Bounded, read-only inspection of single-leaf Finder stores.

mod leaf;
mod properties;

use std::collections::HashSet;

use ds_parser::{FieldKey, FinderViewStyle, FourCC, ValueTag};
use thiserror::Error;

use leaf::Cursor;

const MAX_STORE_BYTES: usize = 64 << 10;
const MAX_RECORDS: usize = 512;
const MAX_NAME_UNITS: usize = 255;
const MAX_PLIST_OBJECTS: usize = 128;
const MAX_PLIST_ENTRIES: usize = 64;
const NAME_UNIT_BYTES: usize = size_of::<u16>();
const MIN_RECORD_BYTES: usize = size_of::<u32>() + 2 * size_of::<[u8; 4]>() + size_of::<u8>();
const LOCATION_BYTES: usize = 16;

/// The byte and count budgets for a Finder inspection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReadLimits {
	/// Maximum bytes in the supplied store, including unused regions.
	pub store_bytes: usize,
	/// Maximum record count.
	pub records: usize,
	/// Maximum length of each record filename in UTF-16 code units.
	pub name_units: usize,
	/// Maximum number of objects in a known binary plist value.
	pub plist_objects: usize,
	/// Maximum number of key-value pairs in each known flat settings dictionary.
	pub plist_entries: usize,
}

impl Default for ReadLimits {
	fn default() -> Self {
		Self {
			store_bytes: MAX_STORE_BYTES,
			records: MAX_RECORDS,
			name_units: MAX_NAME_UNITS,
			plist_objects: MAX_PLIST_OBJECTS,
			plist_entries: MAX_PLIST_ENTRIES,
		}
	}
}

/// The scope of a configured budget failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadBudget {
	StoreBytes,
	Records,
	NameUnits,
	PlistObjects,
	PlistEntries,
}

/// Failure to read the supported store profile.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ReadError {
	#[error("Finder store exceeds its {0:?} budget")]
	Limit(ReadBudget),
	#[error("invalid Finder store header")]
	Header,
	#[error("invalid Finder allocator range or directory")]
	Allocator,
	#[error("Finder allocator has no B-tree reference")]
	MissingTree,
	#[error("Finder store uses an unsupported B-tree shape")]
	UnsupportedTree,
	#[error("Finder store has an inconsistent record count")]
	RecordCount,
	#[error("Finder store ended within a structure")]
	Truncated,
	#[error("invalid Finder record filename")]
	Name,
	#[error("duplicate Finder record identity")]
	DuplicateRecord,
	#[error("unrecognized Finder value type {0:?}")]
	UnsupportedType([u8; 4]),
	#[error("invalid payload for Finder field {0:?}")]
	Payload([u8; 4]),
	#[error("invalid binary Finder settings plist")]
	PlistLayout,
	#[error("Finder settings require a flat binary plist dictionary")]
	PlistProfile,
	#[error("Finder settings contain a nonfinite number")]
	SettingsNumber,
	#[error("cannot decode Finder settings plist")]
	Plist(#[from] plist::Error),
	#[error("cannot reserve Finder report memory")]
	Reserve(#[from] std::collections::TryReserveError),
}

/// Finder record facts in their on-disk order.
#[derive(Debug, PartialEq)]
pub struct Report {
	/// Decoded and opaque records, in B-tree leaf order.
	pub records: Vec<Record>,
}

/// Raw record identity and its admitted semantic value.
#[derive(Debug, PartialEq)]
pub struct Record {
	/// On-disk name. Treat it as untrusted text and escape it in terminal output.
	pub name: String,
	/// Raw four-byte record field tag.
	pub field: [u8; 4],
	/// Raw four-byte payload type tag.
	pub data_type: [u8; 4],
	/// Reported semantics or an opaque record identity.
	pub value: RecordValue,
}

/// Semantic fields admitted by the initial reader.
#[derive(Debug, PartialEq)]
#[non_exhaustive]
pub enum RecordValue {
	Window(Window),
	IconView(IconView),
	View(ViewKind),
	StoreVersion(u32),
	IconPosition {
		x: u32,
		y: u32,
	},
	/// Identity of a record whose known wire type was skipped.
	Opaque,
}

/// The Finder view code, without a host rendering claim.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ViewKind {
	Icon,
	Other([u8; 4]),
}

/// The AppKit window bounds, with a lower-left origin in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowBounds {
	pub x: f64,
	pub y: f64,
	pub width: f64,
	pub height: f64,
}

/// Finder browser window settings emitted by the initial writer.
#[derive(Debug, PartialEq)]
pub struct Window {
	pub bounds: Option<WindowBounds>,
	pub show_sidebar: Option<bool>,
	pub show_toolbar: Option<bool>,
	pub show_status_bar: Option<bool>,
	pub show_path_bar: Option<bool>,
	pub show_tab_view: Option<bool>,
	pub container_show_sidebar: Option<bool>,
	pub preview_pane_visible: Option<bool>,
	pub sidebar_width: Option<f64>,
	/// Names of unreported settings. Their values are omitted.
	pub unreported_keys: Vec<String>,
}

/// Finder icon-view settings emitted by the initial writer.
#[derive(Debug, PartialEq)]
pub struct IconView {
	pub arrangement: Option<String>,
	/// The stored background type. This report does not resolve image aliases.
	pub background_type: Option<i64>,
	pub background_color: Option<[f64; 3]>,
	pub grid_offset: Option<[f64; 2]>,
	pub grid_spacing: Option<f64>,
	pub scroll_position: Option<[f64; 2]>,
	pub icon_size: Option<f64>,
	pub text_size: Option<f64>,
	pub label_on_bottom: Option<bool>,
	pub show_icon_preview: Option<bool>,
	pub show_item_info: Option<bool>,
	pub view_options_version: Option<i64>,
	/// Names of unreported settings. Their values are omitted.
	pub unreported_keys: Vec<String>,
}

/// Inspect the bounded single-leaf profile without consulting a filesystem.
///
/// This reports supported record semantics and opaque record identities. Buddy
/// free lists, other allocator clients, and rendering on a host are outside the
/// report. It does not follow aliases or expose unknown record values.
///
/// # Errors
///
/// Returns an error for a malformed store, an unsupported tree or value shape,
/// or a budget violation.
pub fn inspect(bytes: &[u8], limits: ReadLimits) -> Result<Report, ReadError> {
	if bytes.len() > limits.store_bytes {
		return Err(ReadError::Limit(ReadBudget::StoreBytes));
	}
	let leaf = leaf::read(bytes)?;
	if leaf.record_count > limits.records {
		return Err(ReadError::Limit(ReadBudget::Records));
	}
	if leaf.record_count > leaf.records.len() / MIN_RECORD_BYTES {
		return Err(ReadError::RecordCount);
	}

	let mut records = Vec::new();
	records.try_reserve_exact(leaf.record_count)?;
	let mut names = HashSet::new();
	names.try_reserve(leaf.record_count)?;
	let mut input = Cursor::new(leaf.records);
	for _ in 0..leaf.record_count {
		let name = read_name(&mut input, limits.name_units)?;
		let field = input.array()?;
		let data_type = input.array()?;
		let tag = ValueTag::from_fourcc(FourCC::from(data_type))
			.ok_or(ReadError::UnsupportedType(data_type))?;
		let identity_name = if name.is_ascii() {
			name.to_ascii_lowercase()
		} else {
			name.clone()
		};
		if !names.insert((identity_name, field)) {
			return Err(ReadError::DuplicateRecord);
		}
		let payload = read_payload(&mut input, tag)?;
		let value = report_value(
			FieldKey::from_fourcc(FourCC::from(field)),
			tag,
			payload,
			limits,
		)?;
		records.push(Record {
			name,
			field,
			data_type,
			value,
		});
	}
	Ok(Report { records })
}

fn read_name(input: &mut Cursor<'_>, limit: usize) -> Result<String, ReadError> {
	let count = input.count()?;
	if count > limit {
		return Err(ReadError::Limit(ReadBudget::NameUnits));
	}
	let byte_count = count.checked_mul(NAME_UNIT_BYTES).ok_or(ReadError::Name)?;
	let bytes = input.take(byte_count)?;
	if count == 0 {
		return Err(ReadError::Name);
	}
	let (units, _) = bytes.as_chunks::<NAME_UNIT_BYTES>();
	char::decode_utf16(units.iter().copied().map(u16::from_be_bytes))
		.collect::<Result<String, _>>()
		.map_err(|_| ReadError::Name)
}

fn read_payload<'a>(input: &mut Cursor<'a>, tag: ValueTag) -> Result<&'a [u8], ReadError> {
	match tag {
		ValueTag::Blob => input.counted_bytes(),
		ValueTag::Ustr => {
			let bytes = input
				.count()?
				.checked_mul(NAME_UNIT_BYTES)
				.ok_or(ReadError::Truncated)?;
			input.take(bytes)
		}
		ValueTag::Bool => input.take(size_of::<u8>()),
		ValueTag::Shor | ValueTag::Long | ValueTag::Type => input.take(size_of::<u32>()),
		ValueTag::Comp | ValueTag::Dutc => input.take(size_of::<u64>()),
	}
}

fn report_value(
	field: FieldKey,
	tag: ValueTag,
	payload: &[u8],
	limits: ReadLimits,
) -> Result<RecordValue, ReadError> {
	let mut value = Cursor::new(payload);
	match (field, tag) {
		(FieldKey::bwsp, ValueTag::Blob) => {
			Ok(RecordValue::Window(properties::window(payload, limits)?))
		}
		(FieldKey::icvp, ValueTag::Blob) => {
			Ok(RecordValue::IconView(properties::icons(payload, limits)?))
		}
		(FieldKey::icvl, ValueTag::Type) => {
			let code = value.array()?;
			let view = if code == FinderViewStyle::Icon.fourcc().bytes() {
				ViewKind::Icon
			} else {
				ViewKind::Other(code)
			};
			Ok(RecordValue::View(view))
		}
		(FieldKey::vSrn, ValueTag::Long) => Ok(RecordValue::StoreVersion(value.word()?)),
		(FieldKey::Iloc, ValueTag::Blob) if payload.len() == LOCATION_BYTES => {
			Ok(RecordValue::IconPosition {
				x: value.word()?,
				y: value.word()?,
			})
		}
		(FieldKey::bwsp | FieldKey::icvp | FieldKey::icvl | FieldKey::vSrn | FieldKey::Iloc, _) => {
			Err(ReadError::Payload(field.fourcc().bytes()))
		}
		_ => Ok(RecordValue::Opaque),
	}
}
