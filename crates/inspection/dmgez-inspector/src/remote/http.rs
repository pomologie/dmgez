//! HTTP and HTTPS transport for one explicitly selected image endpoint.

use std::{
	fmt,
	io::{self, Read},
	time::Instant,
};

use ureq::{
	Agent, BodyReader,
	http::{HeaderMap, HeaderName, HeaderValue, StatusCode, Uri, header, uri::Scheme},
};

use super::{BYTES_UNIT, RangeRequest, RangeResponse, RangeTransport, TransportError, decimal};

const MAX_ENDPOINT_BYTES: usize = 8 * 1024;
const MAX_RESPONSE_HEADER_BYTES: usize = 16 * 1024;
const BODY_READ_FAILED: &str = "remote image response body read failed";

/// The supplied URL is outside the admitted HTTP endpoint profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EndpointError;

impl fmt::Display for EndpointError {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		formatter
			.write_str("expected an absolute HTTP(S) image URL without credentials or a fragment")
	}
}

impl std::error::Error for EndpointError {}

/// Direct HTTP(S) transport for an endpoint explicitly authorized by the caller.
///
/// Verifies TLS against WebPKI roots and disables proxies, redirects and idle
/// connection reuse. A partial response must carry one exact `Content-Length`
/// and use identity encoding. This also rejects automatic decompression if an
/// embedding application adds a compression feature to the HTTP client.
/// Use a separate origin policy before accepting URLs from a client or document.
/// Keep dependency trace logging off when using signed URLs.
pub struct HttpTransport {
	agent: Agent,
	endpoint: Uri,
}

impl fmt::Debug for HttpTransport {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		formatter
			.debug_struct("HttpTransport")
			.finish_non_exhaustive()
	}
}

impl HttpTransport {
	/// Admit the endpoint syntax without connecting to it.
	///
	/// # Errors
	/// Returns an endpoint error for unsupported URL schemes, non-absolute URLs,
	/// credentials, fragments and URLs above the endpoint size ceiling.
	pub fn new(endpoint: &str) -> Result<Self, EndpointError> {
		if endpoint.len() > MAX_ENDPOINT_BYTES || endpoint.contains(['#', '\\']) {
			return Err(EndpointError);
		}
		let endpoint = endpoint.parse::<Uri>().map_err(|_| EndpointError)?;
		let authority = endpoint.authority().ok_or(EndpointError)?;
		if !matches!(endpoint.scheme(), Some(scheme) if scheme == &Scheme::HTTP || scheme == &Scheme::HTTPS)
			|| authority.as_str().contains('@')
		{
			return Err(EndpointError);
		}
		let host = authority.host();
		if host.is_empty() || (host != authority.as_str() && authority.port_u16().unwrap_or(0) == 0)
		{
			return Err(EndpointError);
		}
		let agent = Agent::config_builder()
			.proxy(None)
			.max_redirects(0)
			.max_idle_connections(0)
			.http_status_as_error(false)
			.max_response_header_size(MAX_RESPONSE_HEADER_BYTES)
			.build()
			.into();
		Ok(Self { agent, endpoint })
	}
}

impl RangeTransport for HttpTransport {
	type Body = HttpBody;

	fn get(&self, request: RangeRequest<'_>) -> Result<RangeResponse<Self::Body>, TransportError> {
		let deadline = request.deadline();
		let remaining = deadline.saturating_duration_since(Instant::now());
		if remaining.is_zero() {
			return Err(TransportError::Timeout);
		}
		let (start, end) = request.bounds();
		let mut get = self
			.agent
			.get(self.endpoint.clone())
			.header(header::RANGE, format!("{BYTES_UNIT}={start}-{end}"))
			.header(header::ACCEPT_ENCODING, request.accept_encoding());
		if let Some(tag) = request.if_match() {
			let tag = HeaderValue::from_bytes(tag).map_err(|_| TransportError::Failed)?;
			get = get.header(header::IF_MATCH, tag);
		}
		let response = get
			.config()
			.timeout_global(Some(remaining))
			.build()
			.call()
			.map_err(transport_error)?;
		if Instant::now() >= deadline {
			return Err(TransportError::Timeout);
		}

		let (parts, body) = response.into_parts();
		let headers = &parts.headers;
		let content_length = single_header(headers, header::CONTENT_LENGTH)?
			.map(|bytes| {
				std::str::from_utf8(bytes)
					.ok()
					.and_then(decimal)
					.ok_or(TransportError::Failed)
			})
			.transpose()?;
		let content_encoding = single_header(headers, header::CONTENT_ENCODING)?;
		let transfer_encoding = single_header(headers, header::TRANSFER_ENCODING)?;
		if parts.status == StatusCode::PARTIAL_CONTENT
			&& (content_length != Some(end - start + 1)
				|| transfer_encoding.is_some()
				|| content_encoding.is_some_and(|encoding| {
					!encoding.eq_ignore_ascii_case(request.accept_encoding().as_bytes())
				})) {
			return Err(TransportError::Failed);
		}
		Ok(RangeResponse {
			status: parts.status.as_u16(),
			etag: single_header(headers, header::ETAG)?.map(<[u8]>::to_vec),
			content_range: single_header(headers, header::CONTENT_RANGE)?.map(<[u8]>::to_vec),
			content_encoding: content_encoding.map(<[u8]>::to_vec),
			content_length,
			body: HttpBody {
				reader: body.into_with_config().limit(request.body_limit()).reader(),
				deadline,
			},
		})
	}
}

fn single_header(headers: &HeaderMap, name: HeaderName) -> Result<Option<&[u8]>, TransportError> {
	let mut values = headers.get_all(name).into_iter();
	let value = values.next().map(HeaderValue::as_bytes);
	if values.next().is_some() {
		return Err(TransportError::Failed);
	}
	Ok(value)
}

fn transport_error(error: ureq::Error) -> TransportError {
	match error {
		ureq::Error::Timeout(_) => TransportError::Timeout,
		ureq::Error::Io(error) => match error.kind() {
			io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => TransportError::Timeout,
			io::ErrorKind::Interrupted => TransportError::Cancelled,
			_ => TransportError::Failed,
		},
		_ => TransportError::Failed,
	}
}

/// Streaming response body whose public errors omit request metadata.
pub struct HttpBody {
	reader: BodyReader<'static>,
	deadline: Instant,
}

impl Read for HttpBody {
	fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
		if output.is_empty() {
			return Ok(0);
		}
		if Instant::now() >= self.deadline {
			return Err(io::ErrorKind::TimedOut.into());
		}
		let count = self.reader.read(output).map_err(|error| {
			let kind = match error.get_ref().and_then(|cause| cause.downcast_ref()) {
				Some(ureq::Error::Timeout(_)) => io::ErrorKind::TimedOut,
				_ => error.kind(),
			};
			io::Error::new(kind, BODY_READ_FAILED)
		})?;
		if Instant::now() >= self.deadline {
			return Err(io::ErrorKind::TimedOut.into());
		}
		Ok(count)
	}
}

#[cfg(test)]
mod tests;
