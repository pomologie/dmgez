use std::process::{Command, Output};

const BINARY: &str = env!("CARGO_BIN_EXE_dmgez");
const SIGNED_URL: &str = "http://127.0.0.1:1/image.dmg?signature=fixture-secret";
const SECRET: &str = "fixture-secret";

fn inspect(arguments: &[&str]) -> Output {
	Command::new(BINARY)
		.arg("inspect")
		.args(arguments)
		.output()
		.unwrap()
}

#[test]
fn requires_exactly_one_local_image_or_url() {
	for arguments in [&[][..], &["local.dmg", "--url", SIGNED_URL][..]] {
		let result = inspect(arguments);
		assert_eq!(result.status.code(), Some(2));
		assert!(!String::from_utf8_lossy(&result.stderr).contains(SECRET));
	}
}

#[cfg(not(feature = "remote"))]
#[test]
fn reports_remote_as_an_optional_capability() {
	let result = inspect(&["--url", SIGNED_URL]);
	assert!(!result.status.success());
	let error = String::from_utf8_lossy(&result.stderr);
	assert!(error.contains("feature not compiled: remote"), "{error}");
	assert!(!error.contains(SECRET));
}

#[cfg(feature = "remote")]
mod remote {
	use std::{
		io::{self, BufRead, BufReader, Write},
		net::{Ipv4Addr, TcpListener, TcpStream},
		thread,
		time::{Duration, Instant},
	};

	use super::{SECRET, SIGNED_URL, inspect};

	const IMAGE: &[u8] = include_bytes!("../../../tests/fixtures/images/tiny-UDZO.dmg");
	const SIGNED_IMAGE: &[u8] =
		include_bytes!("../../../tests/fixtures/images/tiny-UDZO-adhoc-sha256.dmg");
	const ENTITY_TAG: &str = "\"fixture-v1\"";
	const DEADLINE: Duration = Duration::from_secs(8);
	const METADATA_REQUESTS: usize = 4;

	#[test]
	fn reads_metadata_over_http_and_refuses_a_changed_final_revision() {
		for changed in [false, true] {
			let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
			listener.set_nonblocking(true).unwrap();
			let url = format!(
				"http://{}/image.dmg?signature={SECRET}",
				listener.local_addr().unwrap()
			);
			let fixture = thread::spawn(move || serve_metadata(&listener, IMAGE, false, changed));
			let result = inspect(&["--url", &url, "--json", "--run-limit", "1"]);
			let error = String::from_utf8_lossy(&result.stderr);
			assert!(!error.contains(SECRET));
			if changed {
				assert!(!result.status.success());
				assert!(result.stdout.is_empty());
				assert!(error.contains("source changed"), "{error}");
			} else {
				assert!(result.status.success(), "{error}");
				let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
				assert_eq!(
					report["container"]["physical_bytes"],
					IMAGE.len().to_string()
				);
				assert_eq!(report["coverage"]["payload"], "not_checked");
				assert!(!String::from_utf8_lossy(&result.stdout).contains(SECRET));
			}
			assert!(fixture.join().unwrap() < IMAGE.len());
		}
	}

	#[test]
	fn signed_image_adds_only_the_signature_frame_to_the_metadata_reads() {
		let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
		listener.set_nonblocking(true).unwrap();
		let url = format!(
			"http://{}/image.dmg?signature={SECRET}",
			listener.local_addr().unwrap()
		);
		let fixture = thread::spawn(move || serve_metadata(&listener, SIGNED_IMAGE, true, false));
		let result = inspect(&["--url", &url, "--json"]);
		assert!(result.status.success(), "{result:?}");
		let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
		assert_eq!(
			report["container"]["embedded_signature"]["length_bytes"],
			"351"
		);
		assert_eq!(report["coverage"]["security"], "not_checked");
		assert!(fixture.join().unwrap() < SIGNED_IMAGE.len());
	}

	#[test]
	fn validates_the_requested_window_before_contacting_the_source() {
		let result = inspect(&["--url", SIGNED_URL, "--run-limit", "0"]);
		let error = String::from_utf8_lossy(&result.stderr);
		assert!(!result.status.success());
		assert!(error.contains("run limit must be between"), "{error}");
		assert!(!error.contains(SECRET));
	}

	fn serve_metadata(listener: &TcpListener, image: &[u8], signed: bool, changed: bool) -> usize {
		// Byte intervals are captured in the tiny-UDZO and ad hoc fixture manifests.
		const PAYLOAD_BYTES: usize = 300;
		const SIGNATURE_START: usize = 3084;
		const TRAILER_BYTES: usize = 512;
		let trailer_start = image.len() - TRAILER_BYTES;
		let mut ranges = vec![(0, 0), (trailer_start, image.len() - 1)];
		if signed {
			ranges.push((SIGNATURE_START, trailer_start - 1));
		}
		let xml_end = if signed {
			SIGNATURE_START
		} else {
			trailer_start
		};
		ranges.extend([(PAYLOAD_BYTES, xml_end - 1), (0, 0)]);
		assert_eq!(ranges.len(), METADATA_REQUESTS + usize::from(signed));
		let mut transferred = 0;
		let deadline = Instant::now() + DEADLINE;
		for (index, expected) in ranges.iter().enumerate() {
			let mut stream = accept_until(listener, deadline);
			stream.set_nonblocking(false).unwrap();
			stream.set_read_timeout(Some(DEADLINE)).unwrap();
			let (first, last) = read_range(&stream, index != 0);
			assert_eq!((first, last), *expected);
			if changed && index + 1 == ranges.len() {
				stream.write_all(b"HTTP/1.1 412 Precondition Failed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
				break;
			}
			let body = &image[first..=last];
			write!(stream,
				"HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {first}-{last}/{}\r\nETag: {ENTITY_TAG}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
				image.len(), body.len(),
			).unwrap();
			stream.write_all(body).unwrap();
			transferred += body.len();
		}
		transferred
	}

	fn accept_until(listener: &TcpListener, deadline: Instant) -> TcpStream {
		loop {
			match listener.accept() {
				Ok((stream, _)) => return stream,
				Err(error)
					if error.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline =>
				{
					thread::sleep(Duration::from_millis(5));
				}
				Err(error) => panic!("missing expected fixture request: {error}"),
			}
		}
	}

	fn read_range(stream: &TcpStream, conditional: bool) -> (usize, usize) {
		let mut reader = BufReader::new(stream);
		let mut line = String::new();
		assert_ne!(reader.read_line(&mut line).unwrap(), 0);
		assert!(line.starts_with(&format!("GET /image.dmg?signature={SECRET} ")));
		let mut range = None;
		let mut etag_matches = false;
		let mut identity = false;
		loop {
			line.clear();
			assert_ne!(reader.read_line(&mut line).unwrap(), 0);
			if line == "\r\n" {
				break;
			}
			let (name, value) = line.trim_end().split_once(':').unwrap();
			let value = value.trim();
			match name.to_ascii_lowercase().as_str() {
				"range" => {
					let (first, last) = value
						.strip_prefix("bytes=")
						.unwrap()
						.split_once('-')
						.unwrap();
					range = Some((first.parse().unwrap(), last.parse().unwrap()));
				}
				"accept-encoding" => identity = value == "identity",
				"if-match" => etag_matches = value == ENTITY_TAG,
				_ => (),
			}
		}
		assert!(identity);
		assert_eq!(etag_matches, conditional);
		range.unwrap()
	}
}
