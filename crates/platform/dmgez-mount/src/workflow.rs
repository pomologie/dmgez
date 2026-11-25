//! Operations on recorded, read-only macOS mount sessions.

use std::{
	env, fs, io,
	path::{Path, PathBuf},
};

use thiserror::Error;

use crate::{
	session::{self, Phase, SessionId, SessionRecord, SessionStore},
	system::{self, Observation, Request},
};

const SESSION_ROOT: &str = "dmgez-mount-sessions-v1";
const IMAGE_PATH_KIND: &str = "image";
const MOUNTPOINT_PATH_KIND: &str = "mountpoint";

/// Native workflow failures and any failed attempts to record their effects.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
	/// Failure to access a path or locate the session registry.
	#[error(transparent)]
	Io(#[from] io::Error),
	/// Failure to open or update the recorded session.
	#[error(transparent)]
	Record(#[from] session::Error),
	/// Failure to observe or detach an attachment.
	#[error(transparent)]
	System(#[from] system::Error),
	/// The caller selected a symbolic link as an image or mountpoint path.
	#[error("{role} cannot be a symbolic link: {path:?}")]
	LinkedPath {
		/// Role of the rejected path.
		role: &'static str,
		/// Rejected path.
		path: PathBuf,
	},
	/// The recorded session has no attachment that can safely be detached.
	#[error("the session has no confirmed attachment to detach")]
	NoAttachment,
	/// A native attachment request failed.
	#[error("{source}")]
	Attach {
		/// ID of the record retained for inspection.
		id: SessionId,
		/// Native attachment failure.
		#[source]
		source: system::Error,
	},
	/// A session was detached after saving the mounted state failed.
	#[error("{source}")]
	SaveMount {
		/// ID of the record retained for inspection.
		id: SessionId,
		/// Failure to save the mounted state.
		#[source]
		source: session::Error,
	},
	/// An attach or detach failed and the registry could not record its outcome.
	#[error("{source}. Could not record outcome for {id} ({record})")]
	RecordOutcome {
		/// ID of the affected record.
		id: SessionId,
		/// Native attach or detach error.
		#[source]
		source: system::Error,
		/// Failure to save the resulting state.
		record: session::Error,
		/// Whether the failed operation was an attach.
		attaching: bool,
	},
	/// An immediate cleanup detach failed after saving the mounted state failed.
	#[error("could not save the mount session ({source}). Detach failed ({detach})")]
	Rollback {
		/// ID of the record retained for inspection.
		id: SessionId,
		/// Failure to save the mounted state.
		#[source]
		source: session::Error,
		/// Failure to detach after the record update failed.
		detach: system::Error,
	},
	/// An immediate cleanup detach succeeded but the registry update failed.
	#[error("{source}. Could not record the confirmed detach for {id} ({record})")]
	RecordRollback {
		/// ID of the affected record.
		id: SessionId,
		/// Failure to save the mounted state.
		#[source]
		source: session::Error,
		/// Failure to record the subsequent confirmed detach.
		record: session::Error,
	},
}

impl Error {
	/// Record ID to surface after a failed start.
	#[must_use]
	pub const fn failed_start_id(&self) -> Option<&SessionId> {
		match self {
			Self::Attach { id, .. }
			| Self::SaveMount { id, .. }
			| Self::Rollback { id, .. }
			| Self::RecordRollback { id, .. }
			| Self::RecordOutcome {
				id,
				attaching: true,
				..
			} => Some(id),
			_ => None,
		}
	}
}

/// Opens the default, private registry for a single operation.
///
/// # Errors
/// Returns path, permission, or lock errors from the session registry.
pub fn open_store() -> Result<SessionStore, Error> {
	let directory = env::temp_dir().canonicalize()?.join(SESSION_ROOT);
	Ok(SessionStore::open(&directory)?)
}

/// Attaches a local image and durably records the native outcome.
///
/// # Errors
/// Returns path, native attach, and registry errors. Failed starts may include
/// a retained record that identifies a potentially active attachment.
pub fn start(image: &Path, mountpoint: Option<&Path>) -> Result<SessionRecord, Error> {
	let image = requested_path(image, IMAGE_PATH_KIND)?;
	let mountpoint = mountpoint
		.map(|path| requested_path(path, MOUNTPOINT_PATH_KIND))
		.transpose()?;
	let store = open_store()?;
	let mut session = store.start(&image, mountpoint.as_deref())?;
	let request = Request::new(session.image(), session.mountpoint());
	let attachment = match system::attach(&request) {
		Ok(attachment) => attachment,
		Err(source) => {
			let saved = if source.attachment_may_exist() {
				store.mark_uncertain(&mut session)
			} else {
				store.mark_detached(&mut session)
			};
			let id = session.id().clone();
			return Err(match saved {
				Ok(()) => Error::Attach { id, source },
				Err(record) => Error::RecordOutcome {
					id,
					source,
					record,
					attaching: true,
				},
			});
		}
	};
	if let Err(source) = store.mark_mounted(&mut session, attachment.clone()) {
		let id = session.id().clone();
		if let Err(detach) = system::detach(&attachment) {
			return Err(Error::Rollback { id, source, detach });
		}
		// A failed directory sync may leave the preceding record published.
		let saved = store.get(&id).and_then(|mut current| {
			if matches!(current.phase(), Phase::Detached) {
				return Ok(());
			}
			store.mark_detached(&mut current)
		});
		return Err(match saved {
			Ok(()) => Error::SaveMount { id, source },
			Err(record) => Error::RecordRollback { id, source, record },
		});
	}
	Ok(session)
}

fn requested_path(path: &Path, role: &'static str) -> Result<PathBuf, Error> {
	if fs::symlink_metadata(path)?.is_symlink() {
		return Err(Error::LinkedPath {
			role,
			path: path.to_owned(),
		});
	}
	Ok(path.canonicalize()?)
}

/// Lists saved records in the default registry.
///
/// # Errors
/// Returns registry failures.
pub fn list() -> Result<Vec<SessionRecord>, Error> {
	Ok(open_store()?.list()?)
}

/// Reports the saved record and, when available, a current native observation.
///
/// # Errors
/// Returns registry or native observation failures.
pub fn status(id: &SessionId) -> Result<(SessionRecord, Option<Observation>), Error> {
	let record = open_store()?.get(id)?;
	let observed = if matches!(record.phase(), Phase::Detached) {
		Some(Observation::Detached)
	} else {
		record.last_attachment().map(system::observe).transpose()?
	};
	Ok((record, observed))
}

/// Requests an ordinary, non-forced detach and records the outcome.
///
/// # Errors
/// Returns a registry failure, missing attachment evidence, or a native refusal.
pub fn stop(id: &SessionId) -> Result<SessionRecord, Error> {
	let store = open_store()?;
	let mut record = store.get(id)?;
	if !matches!(record.phase(), Phase::Detached) {
		let attachment = record.last_attachment().ok_or(Error::NoAttachment)?;
		if let Err(source) = system::detach(attachment) {
			return Err(match store.mark_uncertain(&mut record) {
				Ok(()) => source.into(),
				Err(record) => Error::RecordOutcome {
					id: id.clone(),
					source,
					record,
					attaching: false,
				},
			});
		}
		store.mark_detached(&mut record)?;
	}
	Ok(record)
}
