use std::{
	io::{self, Read, Write},
	net::{Ipv4Addr, TcpListener, TcpStream},
	thread::{self, JoinHandle},
	time::{Duration, Instant},
};

use dmgez_udif::{Error, reader::ReadAt};

use super::{HttpTransport, MAX_RESPONSE_HEADER_BYTES};
use crate::remote::{Limits, RemoteError, RemoteSource, TransportError};

const WAIT: Duration = Duration::from_secs(3);
const PARTIAL_RANGE_HEADERS: &str = "HTTP/1.1 206 Partial Content\r\nConnection: close\r\nETag: \"snapshot\"\r\nContent-Range: bytes 0-0/10\r\n";
const CONTENT_LENGTH_HEADER: &str = "Content-Length: 1\r\n";
const END_HEADERS: &str = "\r\n";
const END_REQUEST_HEAD: &[u8] = b"\r\n\r\n";
// One gzip member whose decoded representation is exactly one byte, `a`.
const GZIP_A: &[u8] = &[
	0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0xff, 0x4b, 0x04, 0x00, 0x43, 0xbe, 0xb7,
	0xe8, 0x01, 0x00, 0x00, 0x00,
];

#[test]
fn endpoint_admission_and_errors_do_not_expose_the_url() {
	let valid = "https://example.invalid/input.dmg?key=private-url-token";
	let transport = HttpTransport::new(valid).unwrap();
	assert_eq!(format!("{transport:?}"), "HttpTransport { .. }");
	for invalid in [
		"/input.dmg",
		"file:///input.dmg",
		"http://private-user:private-password@example.invalid/input.dmg",
		"http://private-user@",
		"https://example.invalid:private-port/input.dmg",
		"https://example.invalid:0/input.dmg",
		"https://example.invalid/input.dmg#private-fragment",
	] {
		let error = HttpTransport::new(invalid).unwrap_err();
		assert!(!error.to_string().contains("private"));
	}
}

#[test]
fn sends_ranges_and_a_strong_if_match_to_the_same_resource() {
	let (url, server) = serve(vec![
		Exchange::text(format!(
			"{PARTIAL_RANGE_HEADERS}{CONTENT_LENGTH_HEADER}{END_HEADERS}a"
		)),
		Exchange::text(
			"HTTP/1.1 206 Partial Content\r\nConnection: close\r\nETag: \"snapshot\"\r\nContent-Range: bytes 2-5/10\r\nContent-Length: 4\r\n\r\ncdef",
		),
		Exchange::text(
			"HTTP/1.1 412 Precondition Failed\r\nConnection: close\r\nContent-Length: 0\r\n\r\n",
		),
	]);
	let image = open(&url, Limits::default()).unwrap();
	assert_eq!(image.len(), 10);
	let mut output = [0; 4];
	image.read_exact_at(2, &mut output).unwrap();
	assert_eq!(&output, b"cdef");
	assert!(matches!(image.check_unchanged(), Err(Error::SourceChanged)));
	assert!(matches!(
		image.read_exact_at(2, &mut output),
		Err(Error::SourceChanged)
	));
	let requests = server.join().unwrap();
	assert_eq!(requests.len(), 3);
	for request in &requests {
		assert!(request.starts_with("get /image.dmg?key=placeholder http/1.1\r\n"));
		assert!(request.contains("\r\naccept-encoding: identity\r\n"));
	}
	assert!(requests[0].contains("\r\nrange: bytes=0-0\r\n"));
	assert!(!requests[0].contains("\r\nif-match:"));
	assert!(requests[1].contains("\r\nrange: bytes=2-5\r\n"));
	assert!(requests[1].contains("\r\nif-match: \"snapshot\"\r\n"));
}

#[test]
fn exposes_rejections_without_following_redirects() {
	for status in [200, 302, 416] {
		let (url, server) = serve(vec![Exchange::text(format!(
			"HTTP/1.1 {status} Response\r\nConnection: close\r\nLocation: http://127.0.0.1:0/refused\r\nContent-Length: 0\r\n\r\n"
		))]);
		assert_eq!(
			open(&url, Limits::default()).err(),
			Some(RemoteError::Status(status))
		);
		server.join().unwrap();
	}
}

#[test]
fn rejects_ambiguous_headers_and_bounds_response_headers() {
	for extra in [
		"ETag: \"snapshot\"\r\n",
		"Content-Range: bytes 0-0/10\r\n",
		CONTENT_LENGTH_HEADER,
		"Content-Encoding: identity\r\nContent-Encoding: identity\r\n",
		"Transfer-Encoding: chunked\r\n",
		&format!("X-Size: {}\r\n", "x".repeat(MAX_RESPONSE_HEADER_BYTES)),
	] {
		let (url, server) = serve(vec![Exchange::text(format!(
			"{PARTIAL_RANGE_HEADERS}{CONTENT_LENGTH_HEADER}{extra}{END_HEADERS}a"
		))]);
		assert_eq!(
			open(&url, Limits::default()).err(),
			Some(RemoteError::Transport(TransportError::Failed))
		);
		server.join().unwrap();
	}
}

#[test]
fn refuses_an_encoded_or_inexactly_framed_range() {
	for (headers, body, expected) in [
		(
			format!(
				"Content-Encoding: gzip\r\nContent-Length: {}\r\n",
				GZIP_A.len()
			),
			GZIP_A,
			RemoteError::Transport(TransportError::Failed),
		),
		(
			CONTENT_LENGTH_HEADER.into(),
			b"".as_slice(),
			RemoteError::ContentLength,
		),
		(
			"Transfer-Encoding: chunked\r\n".into(),
			b"1\r\na\r\n0\r\n\r\n".as_slice(),
			RemoteError::Transport(TransportError::Failed),
		),
		(
			"".into(),
			b"a".as_slice(),
			RemoteError::Transport(TransportError::Failed),
		),
		(
			"Content-Length: 2\r\n".into(),
			b"ab".as_slice(),
			RemoteError::Transport(TransportError::Failed),
		),
	] {
		let mut response = Exchange::text(format!("{PARTIAL_RANGE_HEADERS}{headers}{END_HEADERS}"));
		response.prefix.extend_from_slice(body);
		let (url, server) = serve(vec![response]);
		assert_eq!(open(&url, Limits::default()).err(), Some(expected));
		server.join().unwrap();
	}
}

#[test]
fn the_deadline_remains_in_force_after_receiving_headers() {
	let (url, server) = serve(vec![Exchange {
		prefix: format!("{PARTIAL_RANGE_HEADERS}{CONTENT_LENGTH_HEADER}{END_HEADERS}").into_bytes(),
		late_body: Some((Duration::from_millis(600), b'a')),
	}]);
	let limits = Limits {
		max_duration: Duration::from_millis(150),
		..Limits::default()
	};
	assert_eq!(
		open(&url, limits).err(),
		Some(RemoteError::Transport(TransportError::Timeout))
	);
	server.join().unwrap();
}

fn open(url: &str, limits: Limits) -> Result<RemoteSource<HttpTransport>, RemoteError> {
	RemoteSource::open(HttpTransport::new(url).unwrap(), limits)
}

struct Exchange {
	prefix: Vec<u8>,
	late_body: Option<(Duration, u8)>,
}

impl Exchange {
	fn text(text: impl Into<String>) -> Self {
		Self {
			prefix: text.into().into_bytes(),
			late_body: None,
		}
	}
}

fn serve(responses: Vec<Exchange>) -> (String, JoinHandle<Vec<String>>) {
	let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
	listener.set_nonblocking(true).unwrap();
	let endpoint = format!(
		"http://{}/image.dmg?key=placeholder",
		listener.local_addr().unwrap()
	);
	let worker = thread::spawn(move || {
		responses
			.into_iter()
			.map(|exchange| {
				let mut socket = accept(&listener);
				socket.set_nonblocking(false).unwrap();
				socket.set_read_timeout(Some(WAIT)).unwrap();
				socket.set_write_timeout(Some(WAIT)).unwrap();
				let request = request_head(&mut socket);
				socket.write_all(&exchange.prefix).unwrap();
				if let Some((delay, byte)) = exchange.late_body {
					thread::sleep(delay);
					let _ = socket.write_all(&[byte]);
				}
				request
			})
			.collect()
	});
	(endpoint, worker)
}

fn request_head(socket: &mut TcpStream) -> String {
	let mut request = Vec::new();
	while !request.ends_with(END_REQUEST_HEAD) {
		assert!(request.len() < MAX_RESPONSE_HEADER_BYTES);
		let mut byte = [0];
		socket.read_exact(&mut byte).unwrap();
		request.push(byte[0]);
	}
	String::from_utf8(request).unwrap().to_ascii_lowercase()
}

fn accept(listener: &TcpListener) -> TcpStream {
	let deadline = Instant::now() + WAIT;
	loop {
		match listener.accept() {
			Ok((socket, _)) => return socket,
			Err(error)
				if error.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline =>
			{
				thread::sleep(Duration::from_millis(2));
			}
			Err(error) => panic!("local fixture server did not receive a request: {error}"),
		}
	}
}
