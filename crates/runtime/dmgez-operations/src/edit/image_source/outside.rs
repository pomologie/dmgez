use std::{cell::RefCell, io, ops::Range, rc::Rc};

use dmgez_udif::reader::{Image, ReadAt};

pub(super) fn regions<S: ReadAt>(
	image: Image<S>,
	volume: Range<u64>,
) -> (impl io::Read, impl io::Read) {
	let len = image.descriptor().logical_bytes;
	let image = Rc::new(RefCell::new(image));
	(
		Region {
			image: Rc::clone(&image),
			remaining: 0..volume.start,
		},
		Region {
			image,
			remaining: volume.end..len,
		},
	)
}

struct Region<S> {
	image: Rc<RefCell<Image<S>>>,
	remaining: Range<u64>,
}

impl<S: ReadAt> io::Read for Region<S> {
	fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
		let start = self.remaining.start;
		let len = (self.remaining.end - start).min(buffer.len() as u64) as usize;
		self.image
			.borrow_mut()
			.read_exact_at(start, &mut buffer[..len])
			.map_err(io::Error::other)?;
		self.remaining.start += len as u64;
		Ok(len)
	}
}
