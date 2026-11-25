#![cfg(feature = "remote")]

use std::{
	cell::{Cell, RefCell},
	collections::VecDeque,
	io::{self, Cursor, Read},
	rc::Rc,
};

use dmgez_inspector::remote::{
	Limits, RangeRequest, RangeResponse, RangeTransport, RemoteError, RemoteSource, TransportError,
};
use dmgez_udif::{
	Error,
	reader::{Image, Limits as UdifLimits, ReadAt},
};

const ENTITY_TAG: &[u8] = b"\"test-image-v1\"";
const NEXT_ENTITY_TAG: &[u8] = b"\"test-image-v2\"";
const ENTITY_PROBE: Reply = Reply {
	status: 206,
	etag: Some(ENTITY_TAG),
	range: Some(b"bytes 0-0/8"),
	encoding: None,
	length: Some(1),
	body: b"a",
	failure: None,
};
const PAIR: Reply = Reply {
	range: Some(b"bytes 1-2/8"),
	length: Some(2),
	body: b"bc",
	..ENTITY_PROBE
};

#[derive(Clone, Copy)]
struct Reply {
	status: u16,
	etag: Option<&'static [u8]>,
	range: Option<&'static [u8]>,
	encoding: Option<&'static [u8]>,
	length: Option<u64>,
	body: &'static [u8],
	failure: Option<io::ErrorKind>,
}

#[derive(Debug, PartialEq, Eq)]
struct Sent {
	bounds: (u64, u64),
	if_match: Option<Vec<u8>>,
	body_limit: u64,
}

#[derive(Default)]
struct Seen {
	requests: RefCell<Vec<Sent>>,
	body_bytes: Cell<u64>,
}

struct Script {
	replies: RefCell<VecDeque<Reply>>,
	seen: Rc<Seen>,
}

impl Script {
	fn new<const N: usize>(replies: [Reply; N]) -> (Self, Rc<Seen>) {
		let seen = Rc::new(Seen::default());
		let script = Self {
			replies: RefCell::new(replies.into()),
			seen: Rc::clone(&seen),
		};
		(script, seen)
	}
}

struct Body {
	cursor: Cursor<&'static [u8]>,
	seen: Rc<Seen>,
	failure: Option<io::ErrorKind>,
}

impl Read for Body {
	fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
		if let Some(kind) = self.failure {
			return Err(io::Error::new(kind, "untrusted signed URL query"));
		}
		let count = self.cursor.read(output)?;
		self.seen
			.body_bytes
			.set(self.seen.body_bytes.get() + count as u64);
		Ok(count)
	}
}

impl RangeTransport for Script {
	type Body = Body;

	fn get(&self, request: RangeRequest<'_>) -> Result<RangeResponse<Self::Body>, TransportError> {
		record(&self.seen, &request);
		let reply = self
			.replies
			.borrow_mut()
			.pop_front()
			.expect("unexpected network operation");
		Ok(RangeResponse {
			status: reply.status,
			etag: reply.etag.map(<[u8]>::to_vec),
			content_range: reply.range.map(<[u8]>::to_vec),
			content_encoding: reply.encoding.map(<[u8]>::to_vec),
			content_length: reply.length,
			body: Body {
				cursor: Cursor::new(reply.body),
				seen: Rc::clone(&self.seen),
				failure: reply.failure,
			},
		})
	}
}

fn record(seen: &Seen, request: &RangeRequest<'_>) {
	assert_eq!(request.accept_encoding(), "identity");
	assert!(request.deadline() > std::time::Instant::now());
	seen.requests.borrow_mut().push(Sent {
		bounds: request.bounds(),
		if_match: request.if_match().map(<[u8]>::to_vec),
		body_limit: request.body_limit(),
	});
}

#[test]
fn fetches_exact_conditional_ranges_and_revalidates_cached_access() {
	let (script, seen) = Script::new([ENTITY_PROBE, PAIR, ENTITY_PROBE]);
	let source = RemoteSource::open(script, Limits::default()).unwrap();
	assert_eq!(source.len(), 8);
	let mut pair = [0; 2];
	source.read_exact_at(1, &mut pair).unwrap();
	assert_eq!(&pair, b"bc");
	source.check_unchanged().unwrap();
	source.read_exact_at(source.len(), &mut []).unwrap();
	assert!(matches!(
		source.read_exact_at(u64::MAX, &mut pair),
		Err(Error::OutOfRange { .. })
	));
	let requests = seen.requests.borrow();
	assert_eq!(
		requests.as_slice(),
		[
			Sent {
				bounds: (0, 0),
				if_match: None,
				body_limit: 2
			},
			Sent {
				bounds: (1, 2),
				if_match: Some(ENTITY_TAG.to_vec()),
				body_limit: 3
			},
			Sent {
				bounds: (0, 0),
				if_match: Some(ENTITY_TAG.to_vec()),
				body_limit: 2
			},
		]
	);
	assert_eq!(seen.body_bytes.get(), 4);
}

#[test]
fn rejects_admission_without_an_exact_range_and_strong_validator() {
	for (reply, expected) in [
		(
			Reply {
				status: 200,
				..ENTITY_PROBE
			},
			RemoteError::Status(200),
		),
		(
			Reply {
				status: 302,
				..ENTITY_PROBE
			},
			RemoteError::Status(302),
		),
		(
			Reply {
				status: 416,
				..ENTITY_PROBE
			},
			RemoteError::Status(416),
		),
		(
			Reply {
				etag: Some(b"W/\"weak\""),
				..ENTITY_PROBE
			},
			RemoteError::EntityTag,
		),
		(
			Reply {
				etag: Some(b"\"one\", \"two\""),
				..ENTITY_PROBE
			},
			RemoteError::EntityTag,
		),
		(
			Reply {
				etag: None,
				..ENTITY_PROBE
			},
			RemoteError::EntityTag,
		),
		(
			Reply {
				range: Some(b"bytes 0-0/*"),
				..ENTITY_PROBE
			},
			RemoteError::ContentRange,
		),
		(
			Reply {
				range: Some(b"bytes 1-1/8"),
				..ENTITY_PROBE
			},
			RemoteError::ContentRange,
		),
		(
			Reply {
				range: Some(b"bytes 0-18446744073709551615/8"),
				..ENTITY_PROBE
			},
			RemoteError::ContentRange,
		),
		(
			Reply {
				range: Some(b"bytes 0-0/0"),
				..ENTITY_PROBE
			},
			RemoteError::ContentRange,
		),
		(
			Reply {
				encoding: Some(b"gzip"),
				..ENTITY_PROBE
			},
			RemoteError::ContentEncoding,
		),
		(
			Reply {
				length: Some(14),
				..ENTITY_PROBE
			},
			RemoteError::ContentLength,
		),
	] {
		let (script, seen) = Script::new([reply]);
		assert_eq!(
			RemoteSource::open(script, Limits::default()).err(),
			Some(expected)
		);
		assert_eq!(seen.body_bytes.get(), 0);
	}
}

#[test]
fn changed_entity_disables_the_snapshot_even_if_it_returns_later() {
	for reply in [
		Reply {
			etag: Some(NEXT_ENTITY_TAG),
			..PAIR
		},
		Reply {
			range: Some(b"bytes 1-2/9"),
			..PAIR
		},
		Reply {
			status: 412,
			..PAIR
		},
	] {
		let (script, seen) = Script::new([ENTITY_PROBE, reply]);
		let source = RemoteSource::open(script, Limits::default()).unwrap();
		let mut output = [42; 2];
		assert!(matches!(
			source.read_exact_at(1, &mut output),
			Err(Error::SourceChanged)
		));
		assert!(matches!(
			source.check_unchanged(),
			Err(Error::SourceChanged)
		));
		assert_eq!(output, [42; 2]);
		assert_eq!(seen.requests.borrow().len(), 2);
		assert_eq!(seen.body_bytes.get(), 1);
	}
}

#[test]
fn incorrect_body_lengths_and_timeouts_leave_the_output_untouched() {
	for (reply, expected, bytes_read) in [
		(
			Reply { body: b"b", ..PAIR },
			"remote image body length differs from its range",
			2,
		),
		(
			Reply {
				body: b"bcdef",
				length: None,
				..PAIR
			},
			"remote image body length differs from its range",
			4,
		),
		(
			Reply {
				failure: Some(io::ErrorKind::TimedOut),
				..PAIR
			},
			"remote image request timed out",
			1,
		),
	] {
		let (script, seen) = Script::new([ENTITY_PROBE, reply]);
		let source = RemoteSource::open(script, Limits::default()).unwrap();
		let mut output = [42; 2];
		let Error::Io(error) = source.read_exact_at(1, &mut output).unwrap_err() else {
			panic!("expected redacted I/O error");
		};
		assert_eq!(error.to_string(), expected);
		assert_eq!(output, [42; 2]);
		assert_eq!(seen.body_bytes.get(), bytes_read);
	}
	let (script, _) = Script::new([
		ENTITY_PROBE,
		Reply {
			failure: Some(io::ErrorKind::Interrupted),
			..PAIR
		},
	]);
	let source = RemoteSource::open(script, Limits::default()).unwrap();
	assert!(matches!(
		source.read_exact_at(1, &mut [0; 2]),
		Err(Error::Cancelled)
	));
}

#[test]
fn rejects_exhausted_budgets_before_transport_and_charges_failed_requests() {
	for limits in [
		Limits {
			max_range_bytes: 1,
			..Limits::default()
		},
		Limits {
			max_body_bytes: 4,
			..Limits::default()
		},
		Limits {
			max_requests: 1,
			..Limits::default()
		},
	] {
		let (script, seen) = Script::new([ENTITY_PROBE]);
		let source = RemoteSource::open(script, limits).unwrap();
		assert!(matches!(
			source.read_exact_at(1, &mut [0; 2]),
			Err(Error::ResourceLimit(_))
		));
		assert_eq!(seen.requests.borrow().len(), 1);
	}
	let (script, seen) = Script::new([
		ENTITY_PROBE,
		Reply {
			status: 200,
			..PAIR
		},
	]);
	let source = RemoteSource::open(
		script,
		Limits {
			max_body_bytes: 5,
			..Limits::default()
		},
	)
	.unwrap();
	assert!(matches!(
		source.read_exact_at(1, &mut [0; 2]),
		Err(Error::Io(_))
	));
	assert!(matches!(
		source.read_exact_at(1, &mut [0; 2]),
		Err(Error::ResourceLimit(_))
	));
	assert_eq!(seen.requests.borrow().len(), 2);
}

struct Fixture(Rc<Seen>);

const UDZO: &[u8] = include_bytes!("../../../../tests/fixtures/images/tiny-UDZO.dmg");

impl RangeTransport for Fixture {
	type Body = Body;

	fn get(&self, request: RangeRequest<'_>) -> Result<RangeResponse<Self::Body>, TransportError> {
		record(&self.0, &request);
		let (first, last) = request.bounds();
		let start = first as usize;
		let end = last as usize + 1;
		let body = Body {
			cursor: Cursor::new(&UDZO[start..end]),
			seen: Rc::clone(&self.0),
			failure: None,
		};
		Ok(RangeResponse {
			status: 206,
			etag: Some(ENTITY_TAG.to_vec()),
			content_range: Some(format!("bytes {first}-{last}/{}", UDZO.len()).into_bytes()),
			content_encoding: None,
			content_length: None,
			body,
		})
	}
}

#[test]
fn inspects_native_image_with_only_admission_and_metadata_ranges() {
	let seen = Rc::new(Seen::default());
	let source = RemoteSource::open(Fixture(Rc::clone(&seen)), Limits::default()).unwrap();
	let image = Image::open(source, UdifLimits::default()).unwrap();
	let descriptor = image.descriptor();
	let report = dmgez_inspector::inspect(&image, None);
	assert_eq!(report.container.physical_bytes, UDZO.len() as u64);
	assert_eq!(seen.requests.borrow().len(), 3);
	assert_eq!(
		seen.body_bytes.get(),
		1 + dmgez_udif::TRAILER_BYTES as u64
			+ (descriptor.resource_xml.end - descriptor.resource_xml.start)
	);
	assert!(seen.body_bytes.get() < UDZO.len() as u64);
}
