use std::{
	io,
	sync::mpsc::{self, Receiver},
	thread,
};

use dmgez_udif::Cancellation;
use eframe::egui::{Context, ViewportCommand};

use super::{App, Completion};

const WORKER_NAME: &str = "dmgez document";

pub(super) struct Worker {
	pub(super) receiver: Receiver<Completion>,
	pub(super) cancellation: Option<Cancellation>,
}

impl Worker {
	pub(super) fn start(
		ctx: &Context,
		cancellation: Option<Cancellation>,
		work: impl FnOnce() -> Completion + Send + 'static,
	) -> io::Result<Self> {
		let (send, receiver) = mpsc::sync_channel(1);
		let repaint = ctx.clone();
		thread::Builder::new()
			.name(WORKER_NAME.into())
			.spawn(move || {
				let _ = send.send(work());
				repaint.request_repaint();
			})?;
		Ok(Self {
			receiver,
			cancellation,
		})
	}
}

impl Drop for Worker {
	fn drop(&mut self) {
		if let Some(cancellation) = &self.cancellation {
			cancellation.cancel();
		}
	}
}

impl App {
	pub(super) fn defer_close_for_worker(&self, ctx: &Context) -> bool {
		let Some(worker) = &self.worker else {
			return false;
		};
		ctx.send_viewport_cmd(ViewportCommand::CancelClose);
		if let Some(cancellation) = &worker.cancellation {
			cancellation.cancel();
		}
		true
	}
}
