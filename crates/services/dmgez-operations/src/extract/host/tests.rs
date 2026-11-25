//! Host capability coverage from D-READ-05-T01 and D-READ-05-T03.

mod fixture;

use std::{
	cell::Cell,
	fs,
	io::{self, Write},
	num::NonZeroUsize,
	os::unix::fs::symlink,
	path::Path,
};

use dmgez_vfs::volume::ForkKind;

use super::{ForkWriteError, ForkWriteLimits, HostError, StableDestination, stream_fork};
use fixture::{
	CHUNK_BYTES, DATA, DESTINATION, DETACHED, LIMITS, NESTED, RESOURCE, SENTINEL, Scratch,
	assert_children, copy, host_name, with_fork,
};

#[test]
fn streams_exact_session_bound_data_resource_and_empty_bytes() {
	let scratch = Scratch::new();
	let root = scratch.open();
	let directory = root.directory().create_child(&host_name(NESTED)).unwrap();
	let selected = [
		(DESTINATION, ForkKind::Data, DATA),
		("resource", ForkKind::Resource, RESOURCE),
		("empty", ForkKind::Resource, b"".as_slice()),
	];
	for (name, kind, expected) in selected {
		let amount = with_fork(
			DATA,
			Some(expected),
			kind,
			|_, _| Ok(()),
			|fork| directory.publish_fork(&host_name(name), fork, LIMITS, || false),
		)
		.unwrap()
		.unwrap();
		assert_eq!(amount, expected.len().try_into().unwrap());
		assert_eq!(
			fs::read(scratch.destination.join(NESTED).join(name)).unwrap(),
			expected
		);
	}
	let absent = with_fork(
		DATA,
		None,
		ForkKind::Resource,
		|_, _| Ok(()),
		|fork| directory.publish_fork(&host_name("absent"), fork, LIMITS, || false),
	);
	assert!(absent.is_none());
	assert_children(&scratch.destination, &[NESTED]);
	assert_children(
		&scratch.destination.join(NESTED),
		&["empty", DESTINATION, "resource"],
	);
}

#[test]
fn refuses_unqualified_root_path_and_an_existing_symlink_component() {
	let scratch = Scratch::new();
	let alias = scratch.destination.join("alias");
	symlink(&scratch.outside, &alias).unwrap();
	assert!(matches!(
		StableDestination::open(Path::new(NESTED)),
		Err(HostError::RootPath)
	));
	assert!(matches!(
		StableDestination::open(&alias.join("..")),
		Err(HostError::RootPath)
	));
	assert!(matches!(
		StableDestination::open(&alias),
		Err(HostError::OpenRoot(_))
	));
	fs::create_dir(scratch.outside.join(NESTED)).unwrap();
	assert!(matches!(
		StableDestination::open(&alias.join(NESTED)),
		Err(HostError::OpenRoot(_))
	));

	let root = scratch.open();
	let directory = root.directory();
	assert!(matches!(
		directory.create_child(&host_name("alias")),
		Err(HostError::Conflict)
	));
	symlink(
		scratch.outside.join(DESTINATION),
		scratch.destination.join(DESTINATION),
	)
	.unwrap();
	let conflict = copy(
		&directory,
		LIMITS,
		|_, _| panic!("conflict before fork reads"),
		|| false,
	);
	assert!(matches!(
		conflict,
		Err(ForkWriteError::Host(HostError::Conflict))
	));
	assert!(scratch.destination.join(DESTINATION).is_symlink());
	scratch.outside_untouched();
}

#[test]
fn no_clobber_rename_preserves_a_competing_leaf_after_staging() {
	let scratch = Scratch::new();
	let root = scratch.open();
	let result = copy(
		&root.directory(),
		LIMITS,
		|offset, _| {
			if offset == 0 {
				fs::write(scratch.destination.join(DESTINATION), SENTINEL)?;
			}
			Ok(())
		},
		|| false,
	);
	assert!(matches!(
		result,
		Err(ForkWriteError::Host(HostError::Conflict))
	));
	assert_eq!(
		fs::read(scratch.destination.join(DESTINATION)).unwrap(),
		SENTINEL
	);
	assert_children(&scratch.destination, &[DESTINATION]);
}

#[test]
fn refuses_reparented_child_with_or_without_replacement_symlink() {
	for replace in [false, true] {
		let scratch = Scratch::new();
		let root = scratch.open();
		let child = root.directory().create_child(&host_name(NESTED)).unwrap();
		let original = scratch.destination.join(NESTED);
		let moved = scratch.outside.join(DETACHED);
		let result = copy(
			&child,
			LIMITS,
			|offset, _| {
				if offset != 0 {
					fs::rename(&original, &moved)?;
					if replace {
						symlink(&scratch.outside, &original)?;
					}
				}
				Ok(())
			},
			|| false,
		);
		assert!(matches!(
			result,
			Err(ForkWriteError::Host(HostError::Detached))
		));
		assert_children(&moved, &[]);
		scratch.outside_untouched();
	}
}

#[test]
fn refuses_root_replacement_by_different_directory_inode() {
	let scratch = Scratch::new();
	let root = scratch.open();
	let moved = scratch.outside.join(DETACHED);
	let result = copy(
		&root.directory(),
		LIMITS,
		|offset, _| {
			if offset == 0 {
				fs::rename(&scratch.destination, &moved)?;
				fs::create_dir(&scratch.destination)?;
				fs::write(scratch.destination.join(DESTINATION), SENTINEL)?;
			}
			Ok(())
		},
		|| false,
	);
	assert!(matches!(
		result,
		Err(ForkWriteError::Host(HostError::Detached))
	));
	assert_children(&moved, &[]);
	assert_eq!(
		fs::read(scratch.destination.join(DESTINATION)).unwrap(),
		SENTINEL
	);
	scratch.outside_untouched();
}

#[test]
fn source_failure_or_cancellation_abandons_only_the_owned_stage() {
	let scratch = Scratch::new();
	let root = scratch.open();
	let directory = root.directory();
	let source_error = copy(
		&directory,
		LIMITS,
		|offset, chunk| {
			if offset == 0 {
				Ok(())
			} else {
				chunk[0] = u8::MAX;
				Err(io::ErrorKind::UnexpectedEof.into())
			}
		},
		|| false,
	);
	assert!(
		matches!(source_error, Err(ForkWriteError::Source { offset, .. }) if offset == CHUNK_BYTES.get() as u64)
	);
	assert_children(&scratch.destination, &[]);

	let cancel = Cell::new(false);
	let result = copy(
		&directory,
		LIMITS,
		|offset, _| {
			cancel.set(offset != 0);
			Ok(())
		},
		|| cancel.get(),
	);
	assert!(matches!(result, Err(ForkWriteError::Cancelled)));
	assert_children(&scratch.destination, &[]);
	scratch.outside_untouched();
}

#[test]
fn checks_the_source_at_eof_before_publishing_empty_and_nonempty_forks() {
	let scratch = Scratch::new();
	let root = scratch.open();
	let directory = root.directory();
	for bytes in [b"".as_slice(), DATA] {
		let length = bytes.len() as u64;
		let result = with_fork(
			bytes,
			None,
			ForkKind::Data,
			|offset, buffer| {
				if buffer.is_empty() && offset == length {
					return Err(io::Error::other("source changed at EOF"));
				}
				Ok(())
			},
			|fork| directory.publish_fork(&host_name(DESTINATION), fork, LIMITS, || false),
		)
		.unwrap();
		assert!(matches!(result, Err(ForkWriteError::Source { offset, .. }) if offset == length));
		assert_children(&scratch.destination, &[]);
	}
}

struct ShortThenFail<'file> {
	file: &'file mut fs::File,
	first: bool,
}

impl Write for ShortThenFail<'_> {
	fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
		if self.first {
			self.first = false;
			return self.file.write(&bytes[..1]);
		}
		Err(io::ErrorKind::StorageFull.into())
	}

	fn flush(&mut self) -> io::Result<()> {
		self.file.flush()
	}
}

#[test]
fn short_output_write_then_error_abandons_the_stage() {
	let scratch = Scratch::new();
	let root = scratch.open();
	let directory = root.directory();
	let result = with_fork(
		DATA,
		None,
		ForkKind::Data,
		|_, _| Ok(()),
		|fork| {
			directory.publish_with(
				&host_name(DESTINATION),
				fork.len(),
				&mut || false,
				|stage, cancelled| {
					let mut sink = ShortThenFail {
						file: &mut stage.file,
						first: true,
					};
					stream_fork(
						&directory,
						fork,
						&mut [0; CHUNK_BYTES.get()],
						cancelled,
						|bytes| sink.write_all(bytes).map_err(ForkWriteError::Write),
					)
				},
			)
		},
	)
	.unwrap();
	assert!(
		matches!(result, Err(ForkWriteError::Write(error)) if error.kind() == io::ErrorKind::StorageFull)
	);
	assert_children(&scratch.destination, &[]);
}

#[test]
fn refuses_to_write_or_clean_a_staging_name_replaced_during_source_read() {
	let scratch = Scratch::new();
	let root = scratch.open();
	let mut replacement = None;
	let original_name = "moved-stage";
	let result = copy(
		&root.directory(),
		LIMITS,
		|offset, _| {
			if offset != 0 {
				let stage = fs::read_dir(&scratch.destination)?.next().unwrap()?.path();
				fs::rename(&stage, scratch.outside.join(original_name))?;
				fs::write(&stage, SENTINEL)?;
				replacement = Some(stage);
			}
			Ok(())
		},
		|| false,
	);
	assert!(matches!(
		result,
		Err(ForkWriteError::Host(HostError::StageChanged))
	));
	assert_eq!(fs::read(replacement.unwrap()).unwrap(), SENTINEL);
	assert_eq!(
		fs::read(scratch.outside.join(original_name)).unwrap(),
		&DATA[..CHUNK_BYTES.get()]
	);
	assert!(!scratch.destination.join(DESTINATION).exists());
	scratch.outside_untouched();
}

#[test]
fn rejects_excess_fork_length_or_chunk_before_opening_a_stage() {
	let scratch = Scratch::new();
	let root = scratch.open();
	let directory = root.directory();
	for limits in [
		ForkWriteLimits {
			max_fork_bytes: 1,
			..LIMITS
		},
		ForkWriteLimits {
			chunk_bytes: NonZeroUsize::new(super::MAX_CHUNK_BYTES + 1).unwrap(),
			..LIMITS
		},
	] {
		assert!(matches!(
			copy(
				&directory,
				limits,
				|_, _| panic!("preflight must avoid reading"),
				|| false
			),
			Err(ForkWriteError::Limit)
		));
	}
	assert_children(&scratch.destination, &[]);
}
