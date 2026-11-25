use std::collections::VecDeque;

use super::{Location, Page, Request};

const MAX_HISTORY: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Direction {
	Back,
	Forward,
}

#[derive(Default)]
pub(crate) struct History {
	back: VecDeque<Position>,
	forward: VecDeque<Position>,
}

#[derive(PartialEq, Eq)]
struct Position {
	location: Location,
	offset: u64,
}

impl From<&Page> for Position {
	fn from(page: &Page) -> Self {
		Self {
			location: page.location.clone(),
			offset: page.offset,
		}
	}
}

impl History {
	pub(crate) fn available(&self, direction: Direction) -> bool {
		!self.stack(direction).is_empty()
	}

	pub(crate) fn request(&self, direction: Direction) -> Option<Request> {
		let target = self.stack(direction).back()?;
		Some(Request::Page {
			location: target.location.clone(),
			offset: target.offset,
		})
	}

	pub(super) fn arrived(
		&mut self,
		previous: Option<&Page>,
		page: &Page,
		direction: Option<Direction>,
	) -> bool {
		if let Some(direction) = direction {
			let Some(previous) = previous else {
				return false;
			};
			let (from, to) = match direction {
				Direction::Back => (&mut self.back, &mut self.forward),
				Direction::Forward => (&mut self.forward, &mut self.back),
			};
			if !from.back().is_some_and(|target| {
				target.location == page.location && target.offset == page.offset
			}) {
				return false;
			}
			from.pop_back();
			remember(to, previous);
		} else if let Some(previous) = previous.filter(|old| old.location != page.location) {
			self.forward.clear();
			remember(&mut self.back, previous);
		}
		true
	}

	fn stack(&self, direction: Direction) -> &VecDeque<Position> {
		match direction {
			Direction::Back => &self.back,
			Direction::Forward => &self.forward,
		}
	}
}

fn remember(stack: &mut VecDeque<Position>, page: &Page) {
	if stack.len() == MAX_HISTORY {
		stack.pop_front();
	}
	stack.push_back(page.into());
}

#[cfg(test)]
mod tests;
