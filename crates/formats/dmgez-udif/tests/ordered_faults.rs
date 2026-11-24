#![cfg(all(feature = "write", feature = "parallel"))]

use std::{
	cell::{Cell, RefCell},
	io::{self, Read, Write},
};

use dmgez_udif::{
	Error, SECTOR_BYTES,
	writer::{Cancellation, Execution, Profile, WriteOptions, WriteReport, write_image},
};

const CHUNK_BYTES: usize = SECTOR_BYTES as usize;
const SHORT_WRITE_BYTES: usize = CHUNK_BYTES / 2;
const MAX_JOBS: usize = 4;
const MAX_WORKERS: usize = 3;
const INJECTED_FAILURE: &str = "injected scheduler replay I/O failure";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
	Read(usize),
	Write(usize),
	WriteRemainder(usize),
	Eof,
	Resource,
	Trailer,
	Flush,
}

#[derive(Debug, Clone, Copy)]
enum Fault {
	FailAt(Step),
	CancelAfter(Step),
	FailAfterShortWrite(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Event {
	step: Step,
	accepted_bytes: usize,
}

struct Probe {
	fault: Option<Fault>,
	cancellation: Cancellation,
	accepted_bytes: Cell<usize>,
	events: RefCell<Vec<Event>>,
}

impl Probe {
	fn visit(&self, step: Step) -> io::Result<()> {
		self.events.borrow_mut().push(Event {
			step,
			accepted_bytes: self.accepted_bytes.get(),
		});
		match self.fault {
			Some(Fault::FailAt(target)) if target == step => Err(injected_failure()),
			Some(Fault::CancelAfter(target)) if target == step => {
				self.cancellation.cancel();
				Ok(())
			}
			_ => Ok(()),
		}
	}
}

struct Source<'test> {
	input: &'test [u8],
	probe: &'test Probe,
	next_chunk: usize,
}

impl Read for Source<'_> {
	fn read(&mut self, destination: &mut [u8]) -> io::Result<usize> {
		if self.input.is_empty() {
			self.probe.visit(Step::Eof)?;
			return Ok(0);
		}
		assert_eq!(destination.len(), CHUNK_BYTES);
		self.probe.visit(Step::Read(self.next_chunk))?;
		let (chunk, rest) = self.input.split_at(CHUNK_BYTES);
		destination.copy_from_slice(chunk);
		self.input = rest;
		self.next_chunk += 1;
		Ok(CHUNK_BYTES)
	}
}

struct Sink<'test> {
	probe: &'test Probe,
	payload: Vec<u8>,
	remaining_chunks: usize,
	partial: Option<usize>,
	metadata_steps: std::array::IntoIter<Step, 2>,
}

impl Sink<'_> {
	fn accept(&mut self, bytes: &[u8]) -> usize {
		self.payload.extend_from_slice(bytes);
		self.probe.accepted_bytes.set(self.payload.len());
		bytes.len()
	}
}

impl Write for Sink<'_> {
	fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
		if let Some(chunk) = self.partial {
			assert_eq!(bytes.len(), CHUNK_BYTES - SHORT_WRITE_BYTES);
			self.probe.visit(Step::WriteRemainder(chunk))?;
			return Err(injected_failure());
		}

		if self.remaining_chunks != 0 {
			assert_eq!(bytes.len(), CHUNK_BYTES);
			let chunk = usize::from(bytes[0]);
			self.probe.visit(Step::Write(chunk))?;
			self.remaining_chunks -= 1;
			if matches!(self.probe.fault, Some(Fault::FailAfterShortWrite(target)) if target == chunk)
			{
				self.partial = Some(chunk);
				return Ok(self.accept(&bytes[..SHORT_WRITE_BYTES]));
			}
		} else {
			let step = self
				.metadata_steps
				.next()
				.expect("resource and trailer writes");
			self.probe.visit(step)?;
		}
		Ok(self.accept(bytes))
	}

	fn flush(&mut self) -> io::Result<()> {
		self.probe.visit(Step::Flush)
	}
}

struct Replay {
	result: Result<WriteReport, Error>,
	events: Vec<Event>,
	output: Vec<u8>,
}

#[derive(Debug)]
struct Case {
	jobs: usize,
	workers: usize,
}

impl Case {
	fn input(&self) -> Vec<u8> {
		(1..=self.jobs)
			.flat_map(|job| std::iter::repeat_n(u8::try_from(job).unwrap(), CHUNK_BYTES))
			.collect()
	}

	fn budget(&self) -> usize {
		// UDRO charges one raw buffer for each admitted slot.
		self.workers * CHUNK_BYTES
	}

	fn expected_steps(&self) -> Vec<Step> {
		let ordinals: Vec<_> = (1..=self.jobs).collect();
		ordinals
			.chunks(self.workers)
			.flat_map(|batch| {
				batch
					.iter()
					.copied()
					.map(Step::Read)
					.chain(batch.iter().copied().map(Step::Write))
			})
			.chain([Step::Eof, Step::Resource, Step::Trailer, Step::Flush])
			.collect()
	}

	fn run(&self, budget: usize, fault: Option<Fault>) -> Replay {
		let probe = Probe {
			fault,
			cancellation: Cancellation::default(),
			accepted_bytes: Cell::new(0),
			events: RefCell::new(Vec::new()),
		};
		let options = WriteOptions {
			profile: Profile::Udro,
			chunk_bytes: CHUNK_BYTES,
			execution: Execution::Parallel {
				workers: self.workers,
			},
			max_buffer_bytes: budget,
			cancellation: probe.cancellation.clone(),
			..WriteOptions::default()
		};
		let input = self.input();
		let source = Source {
			input: &input,
			probe: &probe,
			next_chunk: 1,
		};
		let mut sink = Sink {
			probe: &probe,
			payload: Vec::new(),
			remaining_chunks: self.jobs,
			partial: None,
			metadata_steps: [Step::Resource, Step::Trailer].into_iter(),
		};
		let result = write_image(source, input.len() as u64, &mut sink, &options);
		Replay {
			result,
			output: sink.payload,
			events: probe.events.into_inner(),
		}
	}

	fn completed(&self) -> Replay {
		let replay = self.run(self.budget(), None);
		let expected_steps = self.expected_steps();
		let actual_steps: Vec<_> = replay.events.iter().map(|event| event.step).collect();
		assert_eq!(actual_steps, expected_steps, "{self:?}");
		assert!(replay.output.starts_with(&self.input()), "{self:?}");
		let report = replay.result.as_ref().expect("complete image");
		assert_eq!(report.raw_runs, self.jobs, "{self:?}");
		assert_eq!(report.accounted_payload_bytes, self.budget(), "{self:?}");
		assert_eq!(
			report.physical_bytes,
			replay.output.len() as u64,
			"{self:?}"
		);
		replay
	}

	fn compare_prefix(
		&self,
		completed: &Replay,
		fault: Fault,
		event_count: usize,
		output_bytes: usize,
	) -> Replay {
		let replay = self.run(self.budget(), Some(fault));
		assert_eq!(
			replay.events,
			completed.events[..event_count],
			"{self:?}, {fault:?}"
		);
		assert_eq!(
			replay.output,
			completed.output[..output_bytes],
			"{self:?}, {fault:?}"
		);
		replay
	}
}

fn cases() -> impl Iterator<Item = Case> {
	(1..=MAX_JOBS).flat_map(|jobs| (1..=MAX_WORKERS).map(move |workers| Case { jobs, workers }))
}

fn injected_failure() -> io::Error {
	io::Error::other(INJECTED_FAILURE)
}

fn assert_injected_failure(result: Result<WriteReport, Error>, case: &Case, fault: Fault) {
	let Err(Error::Io(error)) = result else {
		panic!("expected injected I/O failure for {case:?}, {fault:?}: {result:?}");
	};
	assert_eq!(error.to_string(), INJECTED_FAILURE, "{case:?}, {fault:?}");
}

#[test]
fn ordered_scheduler_replays_at_each_io_boundary() {
	for case in cases() {
		let completed = case.completed();
		for (index, event) in completed.events.iter().enumerate() {
			let fault = Fault::FailAt(event.step);
			let failed = case.compare_prefix(&completed, fault, index + 1, event.accepted_bytes);
			assert_injected_failure(failed.result, &case, fault);

			let event_count = match event.step {
				// The source length probe precedes the next cancellation check.
				Step::Write(job) if job == case.jobs => index + 2,
				// The finalizer checks again after both writes and the flush.
				Step::Resource | Step::Trailer | Step::Flush => completed.events.len(),
				_ => index + 1,
			};
			let output_bytes = completed
				.events
				.get(event_count)
				.map_or(completed.output.len(), |next| next.accepted_bytes);
			let fault = Fault::CancelAfter(event.step);
			let cancelled = case.compare_prefix(&completed, fault, event_count, output_bytes);
			assert!(
				matches!(cancelled.result, Err(Error::Cancelled)),
				"{case:?}, {fault:?}"
			);
		}
	}
}

#[test]
fn partial_sink_failure_stops_before_the_next_batch() {
	for case in cases() {
		let completed = case.completed();
		for (index, event) in completed.events.iter().enumerate() {
			let Step::Write(chunk) = event.step else {
				continue;
			};
			let fault = Fault::FailAfterShortWrite(chunk);
			let replay = case.run(case.budget(), Some(fault));
			let output_bytes = event.accepted_bytes + SHORT_WRITE_BYTES;
			assert_eq!(
				replay.output,
				completed.output[..output_bytes],
				"{case:?}, {fault:?}"
			);
			let tail = Event {
				step: Step::WriteRemainder(chunk),
				accepted_bytes: output_bytes,
			};
			assert_eq!(
				replay.events.split_last(),
				Some((&tail, &completed.events[..=index])),
				"{case:?}, {fault:?}"
			);
			assert_injected_failure(replay.result, &case, fault);
		}
	}
}

#[test]
fn rejected_worker_width_never_reads_or_writes() {
	for case in cases() {
		let replay = case.run(case.budget() - 1, None);
		assert!(
			matches!(replay.result, Err(Error::ResourceLimit(_))),
			"{case:?}"
		);
		assert!(replay.events.is_empty(), "{case:?}");
		assert!(replay.output.is_empty(), "{case:?}");
	}
}
