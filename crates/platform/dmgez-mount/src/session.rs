//! Durable, descriptor-scoped records for cooperating mount commands.

use std::{
	collections::TryReserveError,
	fmt, io,
	path::{Path, PathBuf},
	process,
	str::FromStr,
	time::{SystemTime, UNIX_EPOCH},
};

use rustix::{fs, io::Errno, process::geteuid};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::system::Attachment;

mod directory;
mod wire;

#[cfg(test)]
mod tests;

use directory::{RootDirectory, create_directory, validate_path};

const RECORD_VERSION: u16 = 1;
const INITIAL_REVISION: u64 = 1;
const SESSION_PREFIX: &str = "mount-";
const PENDING_PREFIX: &str = ".preparing-";
const OWNED_MOUNTPOINT: &str = "volume";
const TIMESTAMP_WIDTH: usize = 32;
const PID_WIDTH: usize = 8;
const COUNTER_WIDTH: usize = 2;
const ID_SEPARATOR: char = '-';
const ID_ATTEMPTS: u8 = 16;
const MAX_SESSIONS: usize = 512;
const MAX_DIRECTORY_ENTRIES: usize = 2048;
const SELF_ENTRY: &[u8] = b".";
const PARENT_ENTRY: &[u8] = b"..";

/// A portable session identifier suitable for use as a single directory name.
#[derive(Debug, Clone, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SessionId(String);

impl SessionId {
	/// The validated session identifier in its wire spelling.
	#[must_use]
	pub fn as_str(&self) -> &str {
		&self.0
	}

	fn allocate(timestamp: u128, process: u32, attempt: u8) -> Self {
		Self(format!(
			"{SESSION_PREFIX}{timestamp:0TIMESTAMP_WIDTH$x}{ID_SEPARATOR}{process:0PID_WIDTH$x}{ID_SEPARATOR}{attempt:0COUNTER_WIDTH$x}"
		))
	}
}

impl FromStr for SessionId {
	type Err = Error;

	fn from_str(value: &str) -> Result<Self, Error> {
		let tail = value.strip_prefix(SESSION_PREFIX).ok_or(Error::InvalidId)?;
		let (timestamp, rest) = tail.split_once(ID_SEPARATOR).ok_or(Error::InvalidId)?;
		let (process, counter) = rest.split_once(ID_SEPARATOR).ok_or(Error::InvalidId)?;
		if !is_hex(timestamp, TIMESTAMP_WIDTH)
			|| !is_hex(process, PID_WIDTH)
			|| !is_hex(counter, COUNTER_WIDTH)
		{
			return Err(Error::InvalidId);
		}
		Ok(Self(value.to_owned()))
	}
}

impl TryFrom<String> for SessionId {
	type Error = Error;

	fn try_from(value: String) -> Result<Self, Self::Error> {
		value.parse()
	}
}

impl From<SessionId> for String {
	fn from(id: SessionId) -> Self {
		id.0
	}
}

impl AsRef<str> for SessionId {
	fn as_ref(&self) -> &str {
		self.as_str()
	}
}

impl fmt::Display for SessionId {
	fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
		output.write_str(self.as_str())
	}
}

fn is_hex(value: &str, width: usize) -> bool {
	value.len() == width
		&& value
			.as_bytes()
			.iter()
			.all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

/// The last committed transition. Live status comes from the native backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state", content = "attachment")]
#[non_exhaustive]
pub enum Phase {
	/// The request was recorded before starting the native command.
	Starting,
	/// The native backend confirmed an attachment.
	Mounted(Box<Attachment>),
	/// An attachment or detach result could not be confirmed.
	Uncertain,
	/// The native backend confirmed the attachment is absent.
	Detached,
}

/// One committed session record and its most recent trusted attachment evidence.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionRecord {
	version: u16,
	id: SessionId,
	revision: u64,
	owner: u32,
	process: u32,
	image: PathBuf,
	mountpoint: PathBuf,
	managed_mountpoint: bool,
	phase: Phase,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	last_known_attachment: Option<Box<Attachment>>,
}

impl SessionRecord {
	/// The immutable session identifier.
	#[must_use]
	pub const fn id(&self) -> &SessionId {
		&self.id
	}

	/// Committed revision, for callers that release and later reacquire the registry lock.
	#[must_use]
	pub const fn revision(&self) -> u64 {
		self.revision
	}

	/// The absolute source path supplied for this session.
	#[must_use]
	pub fn image(&self) -> &Path {
		&self.image
	}

	/// The absolute mountpoint path selected for this session.
	#[must_use]
	pub fn mountpoint(&self) -> &Path {
		&self.mountpoint
	}

	/// Whether the mountpoint lives inside the session directory.
	#[must_use]
	pub const fn managed_mountpoint(&self) -> bool {
		self.managed_mountpoint
	}

	/// The last state committed in the session registry.
	#[must_use]
	pub const fn phase(&self) -> &Phase {
		&self.phase
	}

	/// The latest saved attachment evidence, including detached history.
	#[must_use]
	pub fn last_attachment(&self) -> Option<&Attachment> {
		match &self.phase {
			Phase::Mounted(attachment) => Some(attachment),
			_ => self.last_known_attachment.as_deref(),
		}
	}

	fn validate(&self, root: &Path, id: &SessionId) -> Result<(), Error> {
		validate_path(&self.image)?;
		validate_path(&self.mountpoint)?;
		let correct_owned_path = self.mountpoint == root.join(id.as_str()).join(OWNED_MOUNTPOINT);
		if self.version != RECORD_VERSION
			|| self.revision < INITIAL_REVISION
			|| &self.id != id
			|| self.owner != geteuid().as_raw()
			|| self.process == 0
			|| (self.managed_mountpoint && !correct_owned_path)
			|| (!self.managed_mountpoint
				&& (self.mountpoint.starts_with(root) || root.starts_with(&self.mountpoint)))
			|| (matches!(&self.phase, Phase::Starting | Phase::Mounted(_))
				&& self.last_known_attachment.is_some())
			|| self.last_attachment().is_some_and(|attachment| {
				attachment.image() != self.image || attachment.mountpoint() != self.mountpoint
			}) {
			return Err(Error::InvalidRecord);
		}
		Ok(())
	}
}

/// Stable failures when opening, inspecting, or updating the registry.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
	#[error("invalid mount session identifier")]
	InvalidId,
	#[error("a session path must be absolute, normalized, and representable as UTF-8")]
	Path,
	#[error("the explicit mountpoint overlaps the session registry")]
	RegistryOverlap,
	#[error("the registry object is missing required owner-only permissions")]
	Privacy,
	#[error("another mount command holds the session registry lock")]
	Busy,
	#[error("the selected session registry path changed identity")]
	RegistryChanged,
	#[error("mount session does not exist")]
	Missing,
	#[error("mount session record failed identity or schema checks")]
	InvalidRecord,
	#[error("mount session record changed while reading or since it was loaded")]
	RecordChanged,
	#[error("invalid transition for the currently saved mount session")]
	Transition,
	#[error("the mount registry exceeded its entry or record limit")]
	Limit,
	#[error("could not allocate a unique mount session identifier")]
	IdentifierExhausted,
	#[error("the host session clock predates the Unix epoch")]
	Clock(#[from] std::time::SystemTimeError),
	#[error("could not allocate storage for mount session data")]
	Allocation(#[from] TryReserveError),
	#[error("could not decode or encode a mount session record")]
	RecordJson(#[from] serde_json::Error),
	#[error("mount session registry I/O failed")]
	Io(#[from] io::Error),
}

impl From<Errno> for Error {
	fn from(error: Errno) -> Self {
		Self::Io(error.into())
	}
}

/// An exclusively locked, private session registry. Keep one open for a single
/// command's native state changes, then release the lock by dropping the store.
pub struct SessionStore {
	directory: RootDirectory,
}

struct Inventory {
	sessions: Vec<SessionId>,
	used: usize,
	scanned: usize,
}

impl SessionStore {
	/// Opens or creates the final registry directory beneath an existing parent.
	///
	/// # Errors
	/// Rejects unsafe paths, nonprivate directories, and an already-held lock.
	pub fn open(root: &Path) -> Result<Self, Error> {
		Ok(Self {
			directory: RootDirectory::open(root)?,
		})
	}

	/// Commits a starting record before the caller initiates an attachment.
	/// When no mountpoint is supplied, creates an empty private directory inside
	/// the new session. The supplied image and mountpoint paths are absolute.
	///
	/// # Errors
	/// Rejects invalid paths, a full registry, and failures preserving the record.
	pub fn start(&self, image: &Path, mountpoint: Option<&Path>) -> Result<SessionRecord, Error> {
		validate_path(image)?;
		self.directory.check_binding()?;
		if let Some(path) = mountpoint {
			validate_path(path)?;
			let root = self.directory.path();
			if path.starts_with(root) || root.starts_with(path) {
				return Err(Error::RegistryOverlap);
			}
		}
		let inventory = self.inventory()?;
		if inventory.used == MAX_SESSIONS || inventory.scanned == MAX_DIRECTORY_ENTRIES {
			return Err(Error::Limit);
		}
		let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
		let worker = process::id();
		for attempt in 0..ID_ATTEMPTS {
			let id = SessionId::allocate(timestamp, worker, attempt);
			let pending = format!("{PENDING_PREFIX}{id}");
			if self.directory.exists(id.as_str())? {
				continue;
			}
			match self.directory.mkdir(&pending) {
				Ok(()) => {}
				Err(Errno::EXIST) => continue,
				Err(error) => return Err(error.into()),
			}
			let stage = self.directory.private_child(&pending)?;
			let managed_mountpoint = mountpoint.is_none();
			let mountpoint = if let Some(path) = mountpoint {
				path.to_path_buf()
			} else {
				create_directory(&stage, OWNED_MOUNTPOINT)?;
				self.directory
					.path()
					.join(id.as_str())
					.join(OWNED_MOUNTPOINT)
			};
			let record = SessionRecord {
				version: RECORD_VERSION,
				id,
				revision: INITIAL_REVISION,
				owner: geteuid().as_raw(),
				process: worker,
				image: image.to_path_buf(),
				mountpoint,
				managed_mountpoint,
				phase: Phase::Starting,
				last_known_attachment: None,
			};
			record.validate(self.directory.path(), record.id())?;
			wire::write_initial(&stage, &record)?;
			self.directory
				.publish_child(&stage, &pending, record.id().as_str())?;
			return Ok(record);
		}
		Err(Error::IdentifierExhausted)
	}

	/// Reads and checks a previously committed session record.
	///
	/// # Errors
	/// Reports absent, invalid, or unreadable records.
	pub fn get(&self, id: &SessionId) -> Result<SessionRecord, Error> {
		self.directory.check_binding()?;
		let child = self.directory.private_child(id.as_str())?;
		self.read(&child, id).map(|(record, _)| record)
	}

	/// Returns all committed records in session ID order, including history.
	///
	/// # Errors
	/// Reports an invalid record or exhausted registry bounds.
	pub fn list(&self) -> Result<Vec<SessionRecord>, Error> {
		self.directory.check_binding()?;
		self.inventory()?
			.sessions
			.iter()
			.map(|id| self.get(id))
			.collect()
	}

	/// Records a native attachment that matches the starting intent.
	///
	/// # Errors
	/// Rejects stale records, invalid state transitions, and failed commits.
	pub fn mark_mounted(
		&self,
		record: &mut SessionRecord,
		attachment: Attachment,
	) -> Result<(), Error> {
		self.transition(record, Phase::Mounted(Box::new(attachment)))
	}

	/// Records an indeterminate outcome and retains any previous attachment.
	///
	/// # Errors
	/// Rejects stale records, invalid state transitions, and failed commits.
	pub fn mark_uncertain(&self, record: &mut SessionRecord) -> Result<(), Error> {
		self.transition(record, Phase::Uncertain)
	}

	/// Records absence confirmed by the caller's native observation.
	/// All session history and paths remain available for inspection.
	///
	/// # Errors
	/// Rejects stale records, invalid state transitions, and failed commits.
	pub fn mark_detached(&self, record: &mut SessionRecord) -> Result<(), Error> {
		self.transition(record, Phase::Detached)
	}

	fn read(
		&self,
		child: &rustix::fd::OwnedFd,
		id: &SessionId,
	) -> Result<(SessionRecord, fs::Stat), Error> {
		let (record, stamp) = wire::read(child)?;
		record.validate(self.directory.path(), id)?;
		self.directory.check_child(id.as_str(), child)?;
		Ok((record, stamp))
	}

	fn transition(&self, record: &mut SessionRecord, phase: Phase) -> Result<(), Error> {
		self.directory.check_binding()?;
		let child = self.directory.private_child(record.id.as_str())?;
		let (mut current, stamp) = self.read(&child, &record.id)?;
		if current.revision != record.revision {
			return Err(Error::RecordChanged);
		}
		if matches!(&current.phase, Phase::Detached)
			|| (matches!(&phase, Phase::Mounted(_)) && !matches!(&current.phase, Phase::Starting))
		{
			return Err(Error::Transition);
		}
		if let Phase::Mounted(attachment) = std::mem::replace(&mut current.phase, phase) {
			current.last_known_attachment = Some(attachment);
		}
		current.revision = current.revision.checked_add(1).ok_or(Error::Limit)?;
		current.validate(self.directory.path(), &record.id)?;
		self.directory.check_binding()?;
		self.directory.check_child(record.id.as_str(), &child)?;
		wire::replace(&child, &current, &stamp)?;
		self.directory.check_child(record.id.as_str(), &child)?;
		*record = current;
		Ok(())
	}

	fn inventory(&self) -> Result<Inventory, Error> {
		let mut sessions = Vec::new();
		let mut used = 0;
		let mut scanned = 0;
		for (entry_index, entry) in fs::Dir::read_from(self.directory.descriptor())?.enumerate() {
			if entry_index >= MAX_DIRECTORY_ENTRIES {
				return Err(Error::Limit);
			}
			scanned += 1;
			let entry = entry?;
			let name = entry.file_name().to_bytes();
			if name == SELF_ENTRY || name == PARENT_ENTRY {
				continue;
			}
			if name.starts_with(SESSION_PREFIX.as_bytes()) {
				let id = std::str::from_utf8(name)
					.map_err(|_| Error::InvalidId)?
					.parse()?;
				sessions.try_reserve(1)?;
				sessions.push(id);
			} else if !name.starts_with(PENDING_PREFIX.as_bytes()) {
				continue;
			}
			used += 1;
			if used > MAX_SESSIONS {
				return Err(Error::Limit);
			}
		}
		sessions.sort_unstable();
		Ok(Inventory {
			sessions,
			used,
			scanned,
		})
	}
}
