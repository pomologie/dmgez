use std::{
	collections::{HashMap, hash_map::Entry},
	io,
	sync::{Arc, Mutex},
};

use rmcp::{
	RoleServer,
	model::{
		ClientJsonRpcMessage, ClientNotification, JsonRpcMessage, RequestId, ServerJsonRpcMessage,
	},
	transport::{Transport, async_rw::AsyncRwTransport},
};
use tokio::{
	io::{stdin, stdout},
	sync::{OwnedSemaphorePermit, Semaphore},
};

pub(super) const MAX_REQUEST_BYTES: usize = 32 << 10;
pub(super) const MAX_IN_FLIGHT: usize = 32;

pub(super) fn stdio() -> impl Transport<RoleServer, Error = io::Error> {
	Stdio {
		inner: AsyncRwTransport::new_server(stdin(), stdout())
			.with_max_line_length(MAX_REQUEST_BYTES),
		slots: Arc::new(Semaphore::new(MAX_IN_FLIGHT)),
		pending: Arc::default(),
		initialized: false,
	}
}

struct Request {
	_slot: OwnedSemaphorePermit,
	cancelled: bool,
}

struct Stdio<T> {
	inner: T,
	slots: Arc<Semaphore>,
	pending: Arc<Mutex<HashMap<RequestId, Request>>>,
	initialized: bool,
}

impl<T: Transport<RoleServer, Error = io::Error>> Transport<RoleServer> for Stdio<T> {
	type Error = io::Error;

	fn send(
		&mut self,
		message: ServerJsonRpcMessage,
	) -> impl Future<Output = io::Result<()>> + Send + 'static {
		let reply_id = match &message {
			JsonRpcMessage::Response(reply) => Some(reply.id.clone()),
			JsonRpcMessage::Error(reply) => reply.id.clone(),
			_ => None,
		};
		let pending = self.pending.clone();
		let slots = self.slots.clone();
		let sent = self.inner.send(message);
		async move {
			let result = sent.await;
			if result.is_err() {
				slots.close();
			}
			if let Some(id) = reply_id {
				let mut pending = pending
					.lock()
					.map_err(|_| io::Error::other("request budget poisoned"))?;
				pending.remove(&id);
			}
			result
		}
	}

	async fn receive(&mut self) -> Option<ClientJsonRpcMessage> {
		// Hold the slot until the matching reply reaches the pipe. A peer that
		// stops reading cannot make the server keep queuing requests or replies.
		let slot = self.slots.clone().acquire_owned().await.ok()?;
		loop {
			let message = self.inner.receive().await?;
			match &message {
				JsonRpcMessage::Request(request) => {
					let mut pending = self.pending.lock().ok()?;
					let Entry::Vacant(entry) = pending.entry(request.id.clone()) else {
						return None;
					};
					entry.insert(Request {
						_slot: slot,
						cancelled: false,
					});
					return Some(message);
				}
				JsonRpcMessage::Notification(notification) => match &notification.notification {
					ClientNotification::InitializedNotification(_) if !self.initialized => {
						self.initialized = true;
						return Some(message);
					}
					ClientNotification::CancelledNotification(notification) => {
						let mut pending = self.pending.lock().ok()?;
						if let Some(id) = &notification.params.request_id
							&& let Some(request) = pending.get_mut(id)
							&& !request.cancelled
						{
							request.cancelled = true;
							return Some(message);
						}
					}
					_ => (),
				},
				_ => (),
			}
		}
	}

	async fn close(&mut self) -> io::Result<()> {
		self.inner.close().await
	}
}
