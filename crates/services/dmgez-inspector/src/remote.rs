//! Bounded HTTP range semantics for an explicitly supplied transport.

use std::{
	fmt,
	io::{self, Read},
	ops::Range,
	sync::Mutex,
	time::{Duration, Instant},
};

use dmgez_udif::{Error, reader::ReadAt};

const PARTIAL_CONTENT: u16 = 206;
const PRECONDITION_FAILED: u16 = 412;
const BYTES_UNIT: &str = "bytes";
const IDENTITY_ENCODING: &str = "identity";
const MAX_ETAG_BYTES: usize = 1024;
const MAX_CONTENT_RANGE_BYTES: usize = 128;
const IDENTITY_PROBE: Range<u64> = 0..1;
const EOF_PROBE_BYTES: u64 = 1;
const DEFAULT_MAX_RANGE_BYTES: usize = 16 << 20;
const DEFAULT_MAX_BODY_BYTES: u64 = 64 << 20;
const DEFAULT_MAX_REQUESTS: u64 = 512;
const DEFAULT_MAX_DURATION: Duration = Duration::from_secs(30);

/// Session budgets for the remote source and its underlying transport.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
	/// Largest logical byte range accepted for one call.
	pub max_range_bytes: usize,
	/// Total reserved response body bytes, including EOF checks and failed requests.
	pub max_body_bytes: u64,
	/// Number of requests, including admission and cached-read checks.
	pub max_requests: u64,
	/// Total elapsed time starting at admission, including transport reads.
	pub max_duration: Duration,
}

impl Default for Limits {
	fn default() -> Self {
		Self {
			max_range_bytes: DEFAULT_MAX_RANGE_BYTES,
			max_body_bytes: DEFAULT_MAX_BODY_BYTES,
			max_requests: DEFAULT_MAX_REQUESTS,
			max_duration: DEFAULT_MAX_DURATION,
		}
	}
}

/// One HTTP GET for a nonempty inclusive wire range.
pub struct RangeRequest<'a> {
	range: Range<u64>,
	if_match: Option<&'a [u8]>,
	body_limit: u64,
	deadline: Instant,
}

impl RangeRequest<'_> {
	/// First and last byte to send in the `Range: bytes=...` header.
	#[must_use]
	pub const fn bounds(&self) -> (u64, u64) {
		(self.range.start, self.range.end - 1)
	}

	/// Verbatim optional strong entity tag to send in `If-Match`.
	#[must_use]
	pub const fn if_match(&self) -> Option<&[u8]> {
		self.if_match
	}

	/// Value to send in `Accept-Encoding` for every request.
	#[must_use]
	pub const fn accept_encoding(&self) -> &'static str {
		IDENTITY_ENCODING
	}

	/// Maximum representation bytes permitted, including one EOF check byte.
	#[must_use]
	pub const fn body_limit(&self) -> u64 {
		self.body_limit
	}

	/// Absolute deadline for connection, response headers and body reads.
	#[must_use]
	pub const fn deadline(&self) -> Instant {
		self.deadline
	}
}

/// The original status and unmodified range headers, with a streaming body.
pub struct RangeResponse<B> {
	pub status: u16,
	pub etag: Option<Vec<u8>>,
	pub content_range: Option<Vec<u8>>,
	pub content_encoding: Option<Vec<u8>>,
	pub content_length: Option<u64>,
	pub body: B,
}

/// A transport tied to one explicitly selected HTTP resource.
///
/// Send each request to the same resource without retries, redirects, or
/// content decompression. Enforce origin authorization, verified TLS where
/// applicable, bounded header parsing and the supplied absolute deadline in
/// both `get` and the body reader. Reject duplicated ETag, Content-Range,
/// Content-Encoding and Content-Length headers. Expose a streaming body that
/// reads no more than the requested body limit and does not synthesize EOF at
/// that limit. The source never accepts URL or credential data from the transport.
/// A streaming reader can signal cancellation with `io::ErrorKind::Interrupted`.
pub trait RangeTransport {
	type Body: Read;

	/// Fetch headers for one request without consuming its representation body.
	///
	/// # Errors
	/// Returns a redacted error if the operation was cancelled, failed or timed out.
	fn get(&self, request: RangeRequest<'_>) -> Result<RangeResponse<Self::Body>, TransportError>;
}

/// A transport failure without endpoint, request or credential data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportError {
	Cancelled,
	Timeout,
	Failed,
}

/// A rejected remote source or range request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RemoteError {
	Limits,
	Status(u16),
	ContentRange,
	ContentEncoding,
	ContentLength,
	EntityTag,
	Changed,
	Transport(TransportError),
}

impl fmt::Display for RemoteError {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::Limits => f.write_str("remote image request exceeds configured limits"),
			Self::Status(status) => write!(f, "unexpected remote range status {status}"),
			Self::ContentRange => f.write_str("remote image returned an invalid byte range"),
			Self::ContentEncoding => f.write_str("remote image returned encoded content"),
			Self::ContentLength => f.write_str("remote image body length differs from its range"),
			Self::EntityTag => f.write_str("remote image did not supply a strong entity tag"),
			Self::Changed => f.write_str("remote image changed after admission"),
			Self::Transport(TransportError::Cancelled) => {
				f.write_str("remote image request cancelled")
			}
			Self::Transport(TransportError::Timeout) => {
				f.write_str("remote image request timed out")
			}
			Self::Transport(TransportError::Failed) => f.write_str("remote image transport failed"),
		}
	}
}

impl std::error::Error for RemoteError {}

/// A finite range-readable image bound to an exact length and strong entity tag.
pub struct RemoteSource<T> {
	transport: T,
	entity: Entity,
	max_range_bytes: usize,
	state: Mutex<State>,
}

struct State {
	requests: u64,
	body_bytes: u64,
	deadline: Instant,
	changed: bool,
}

#[derive(PartialEq, Eq)]
struct Entity {
	length: u64,
	etag: Vec<u8>,
}

impl<T: RangeTransport> RemoteSource<T> {
	/// Admit a resource with an exact one-byte request and a strong entity tag.
	///
	/// # Errors
	/// Returns a configured-limit, transport or protocol error for a source that
	/// fails the admission request. Empty resources are outside this profile.
	pub fn open(transport: T, limits: Limits) -> Result<Self, RemoteError> {
		let deadline = Instant::now()
			.checked_add(limits.max_duration)
			.ok_or(RemoteError::Limits)?;
		let mut state = State {
			requests: limits.max_requests,
			body_bytes: limits.max_body_bytes,
			deadline,
			changed: false,
		};
		if limits.max_range_bytes == 0 || limits.max_duration.is_zero() {
			return Err(RemoteError::Limits);
		}
		let mut byte = [0];
		let entity = state.fetch(&transport, IDENTITY_PROBE, None, &mut byte)?;
		Ok(Self {
			transport,
			entity,
			max_range_bytes: limits.max_range_bytes,
			state: Mutex::new(state),
		})
	}

	fn read_range(&self, range: Range<u64>, output: &mut [u8]) -> Result<(), RemoteError> {
		let mut state = self
			.state
			.lock()
			.map_err(|_| RemoteError::Transport(TransportError::Failed))?;
		if state.changed {
			return Err(RemoteError::Changed);
		}
		if output.len() > self.max_range_bytes {
			return Err(RemoteError::Limits);
		}
		if output.is_empty() {
			return Ok(());
		}
		match state.fetch(&self.transport, range, Some(&self.entity), output) {
			Ok(_) => Ok(()),
			Err(error) => {
				state.changed = error == RemoteError::Changed;
				Err(error)
			}
		}
	}
}

impl<T: RangeTransport> ReadAt for RemoteSource<T> {
	fn len(&self) -> u64 {
		self.entity.length
	}

	fn check_unchanged(&self) -> Result<(), Error> {
		self.read_range(IDENTITY_PROBE, &mut [0])
			.map_err(udif_error)
	}

	fn read_exact_at(&self, offset: u64, output: &mut [u8]) -> Result<(), Error> {
		let len = u64::try_from(output.len()).map_err(|_| Error::ResourceLimit("remote range"))?;
		let end = offset
			.checked_add(len)
			.filter(|end| *end <= self.len())
			.ok_or(Error::OutOfRange {
				offset,
				len,
				disk_len: self.len(),
			})?;
		self.read_range(offset..end, output).map_err(udif_error)
	}
}

impl State {
	fn fetch<T: RangeTransport>(
		&mut self,
		transport: &T,
		range: Range<u64>,
		identity: Option<&Entity>,
		output: &mut [u8],
	) -> Result<Entity, RemoteError> {
		check_deadline(self.deadline)?;
		let body_limit = (range.end - range.start)
			.checked_add(EOF_PROBE_BYTES)
			.ok_or(RemoteError::Limits)?;
		if self.requests == 0 || self.body_bytes < body_limit {
			return Err(RemoteError::Limits);
		}
		self.requests -= 1;
		self.body_bytes -= body_limit;
		let response = transport
			.get(RangeRequest {
				range: range.clone(),
				if_match: identity.map(|entity| entity.etag.as_slice()),
				body_limit,
				deadline: self.deadline,
			})
			.map_err(RemoteError::Transport)?;
		check_deadline(self.deadline)?;
		if response.status == PRECONDITION_FAILED && identity.is_some() {
			return Err(RemoteError::Changed);
		}
		if response.status != PARTIAL_CONTENT {
			return Err(RemoteError::Status(response.status));
		}
		if response
			.content_encoding
			.as_deref()
			.is_some_and(|encoding| !encoding.eq_ignore_ascii_case(IDENTITY_ENCODING.as_bytes()))
		{
			return Err(RemoteError::ContentEncoding);
		}
		let (returned, length) = response
			.content_range
			.as_deref()
			.and_then(content_range)
			.ok_or(RemoteError::ContentRange)?;
		let etag = response
			.etag
			.filter(|tag| strong_etag(tag))
			.ok_or(RemoteError::EntityTag)?;
		let entity = Entity { length, etag };
		if identity.is_some_and(|expected| expected != &entity) {
			return Err(RemoteError::Changed);
		}
		if returned != range {
			return Err(RemoteError::ContentRange);
		}
		if response
			.content_length
			.is_some_and(|declared| declared != range.end - range.start)
		{
			return Err(RemoteError::ContentLength);
		}
		read_body(response.body, output, self.deadline)?;
		Ok(entity)
	}
}

// RFC 9110 §§8.8.3, 14.4. Require a known complete length and one strong tag.
fn content_range(header: &[u8]) -> Option<(Range<u64>, u64)> {
	if header.len() > MAX_CONTENT_RANGE_BYTES {
		return None;
	}
	let (unit, interval) = std::str::from_utf8(header).ok()?.split_once(' ')?;
	if !unit.eq_ignore_ascii_case(BYTES_UNIT) {
		return None;
	}
	let (positions, length) = interval.split_once('/')?;
	let (first, last) = positions.split_once('-')?;
	let start = decimal(first)?;
	let end = decimal(last)?.checked_add(1)?;
	let length = decimal(length)?;
	(start < end && end <= length).then_some((start..end, length))
}

fn decimal(value: &str) -> Option<u64> {
	if value.bytes().all(|byte| byte.is_ascii_digit()) {
		return value.parse().ok();
	}
	None
}

fn strong_etag(value: &[u8]) -> bool {
	if value.len() > MAX_ETAG_BYTES {
		return false;
	}
	matches!(value, [b'"', tag @ .., b'"'] if tag.iter().all(|&byte| {
		matches!(byte, b'!' | b'#'..=b'~' | 0x80..=0xff)
	}))
}

fn read_body(mut body: impl Read, output: &mut [u8], deadline: Instant) -> Result<(), RemoteError> {
	let mut buffer = Vec::new();
	buffer
		.try_reserve_exact(output.len())
		.map_err(|_| RemoteError::Limits)?;
	buffer.resize(output.len(), 0);
	let mut pending = buffer.as_mut_slice();
	while !pending.is_empty() {
		check_deadline(deadline)?;
		let count = body.read(pending).map_err(body_error)?;
		if count == 0 || count > pending.len() {
			return Err(RemoteError::ContentLength);
		}
		pending = &mut pending[count..];
	}
	check_deadline(deadline)?;
	if body.read(&mut [0]).map_err(body_error)? != 0 {
		return Err(RemoteError::ContentLength);
	}
	check_deadline(deadline)?;
	output.copy_from_slice(&buffer);
	Ok(())
}

fn check_deadline(deadline: Instant) -> Result<(), RemoteError> {
	if Instant::now() >= deadline {
		return Err(RemoteError::Transport(TransportError::Timeout));
	}
	Ok(())
}

fn body_error(error: io::Error) -> RemoteError {
	match error.kind() {
		io::ErrorKind::UnexpectedEof => RemoteError::ContentLength,
		io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => {
			RemoteError::Transport(TransportError::Timeout)
		}
		io::ErrorKind::Interrupted => RemoteError::Transport(TransportError::Cancelled),
		_ => RemoteError::Transport(TransportError::Failed),
	}
}

fn udif_error(error: RemoteError) -> Error {
	match error {
		RemoteError::Changed => Error::SourceChanged,
		RemoteError::Limits => Error::ResourceLimit("remote image range requests"),
		RemoteError::Transport(TransportError::Cancelled) => Error::Cancelled,
		RemoteError::Transport(TransportError::Timeout) => {
			io::Error::new(io::ErrorKind::TimedOut, error).into()
		}
		_ => io::Error::other(error).into(),
	}
}
