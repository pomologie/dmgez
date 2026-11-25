use std::{fs, sync::mpsc};

use super::*;

#[test]
fn asynchronous_failure_and_cancel_leave_previous_document_available() {
	let mut app = App::new(&Context::default(), None);
	let scratch = tempfile::tempdir().unwrap();
	let file = scratch.path().join("retained.toml");
	fs::write(
		&file,
		"schema_version=1\n[image]\nname='Original'\noutput='build.dmg'",
	)
	.unwrap();
	app.finish(open_outcome(file.clone()));
	app.finish(Completion::Cancelled);
	app.finish(open_outcome(file.with_extension("exe")));
	assert_eq!(
		app.opened.as_ref().unwrap().path,
		file.canonicalize().unwrap()
	);
	assert!(app.error.is_some());
}

#[test]
fn cancelled_check_discards_a_completed_result_awaiting_delivery() {
	const UDZO: &[u8] = include_bytes!("../../../../tests/fixtures/images/tiny-UDZO.dmg");
	let mut app = App::new(&Context::default(), None);
	let scratch = tempfile::tempdir().unwrap();
	let file = scratch.path().join("retained.dmg");
	fs::write(&file, UDZO).unwrap();
	app.finish(open_outcome(file));
	let Some(Opened {
		document: Document::Image(image),
		..
	}) = &app.opened
	else {
		panic!("expected image tab");
	};
	let cancellation = Cancellation::default();
	let result = image.snapshot.verify(&cancellation);
	assert!(result.passed());
	let (send, receiver) = mpsc::sync_channel(1);
	send.send(Completion::ImageChecked(Box::new(result)))
		.unwrap();
	cancellation.cancel();
	app.worker = Some(Worker {
		receiver,
		cancellation: Some(cancellation),
	});
	app.poll();
	assert!(app.worker.is_none());
	let Some(Opened {
		document: Document::Image(image),
		..
	}) = &app.opened
	else {
		panic!("expected image tab");
	};
	assert!(image.verification.is_none());
}
