use dmgez_vfs::{Error as DiskError, LogicalDisk, Window};

use crate::{ByteRange, DiagnosticDetail, ErrorCategory};

use super::{
	Failure, FilesystemCheckError, FilesystemOptions, FilesystemReadError, FilesystemSource,
};

pub(crate) struct ReadBudget {
	max_requests: u64,
	max_bytes: u64,
	pub(super) charged_requests: u64,
	pub(super) charged_bytes: u64,
	failure: Option<(FilesystemReadError, Failure)>,
}

impl ReadBudget {
	pub(crate) const fn new(limits: FilesystemOptions) -> Self {
		Self {
			max_requests: limits.max_read_requests,
			max_bytes: limits.max_read_bytes,
			charged_requests: 0,
			charged_bytes: 0,
			failure: None,
		}
	}

	pub(super) fn source<'budget, 'window, S: LogicalDisk>(
		&'budget mut self,
		window: Window<'window, S>,
	) -> BudgetedSource<'budget, 'window, S> {
		BudgetedSource {
			window,
			budget: self,
		}
	}

	fn charge(&mut self, length: u64) -> Result<(), FilesystemReadError> {
		if let Some((error, _)) = &self.failure {
			return Err(*error);
		}
		let bytes = self
			.charged_bytes
			.checked_add(length)
			.filter(|&bytes| bytes <= self.max_bytes);
		if let Some(bytes) = bytes
			&& self.charged_requests < self.max_requests
		{
			self.charged_bytes = bytes;
			self.charged_requests += 1;
			return Ok(());
		}
		self.reject(
			FilesystemReadError::Budget,
			Failure::category(ErrorCategory::ResourceLimit),
		)
	}

	fn reject(
		&mut self,
		error: FilesystemReadError,
		failure: Failure,
	) -> Result<(), FilesystemReadError> {
		self.failure = Some((error, failure));
		Err(error)
	}
}

pub(super) struct BudgetedSource<'budget, 'window, S> {
	window: Window<'window, S>,
	budget: &'budget mut ReadBudget,
}

impl<S: LogicalDisk> BudgetedSource<'_, '_, S> {
	pub(super) fn finish<T>(
		&mut self,
		result: Result<T, FilesystemCheckError>,
	) -> Result<T, Failure> {
		match self.budget.failure.take() {
			Some((_, failure)) => Err(failure),
			None => result.map_err(Failure::checker),
		}
	}

	pub(super) fn revalidate(&mut self) -> Result<(), Failure> {
		// This identity check is independent of the engine's read budget.
		let end = self.window.len();
		Ok(self.window.read_exact_at(end, &mut [])?)
	}
}

impl<S: LogicalDisk> FilesystemSource for BudgetedSource<'_, '_, S> {
	fn len(&self) -> u64 {
		self.window.len()
	}

	fn read_exact_at(&mut self, offset: u64, output: &mut [u8]) -> Result<(), FilesystemReadError> {
		self.budget.charge(output.len() as u64)?;
		self.window.read_exact_at(offset, output).or_else(|error| {
			if let DiskError::OutOfRange {
				offset,
				len,
				available,
			} = error
			{
				let mut failure = Failure::category(ErrorCategory::Corrupt);
				failure.diagnostic.detail = Some(DiagnosticDetail::FilesystemWindowRange {
					requested: ByteRange {
						offset_bytes: offset,
						length_bytes: len,
					},
					available_bytes: available,
				});
				self.budget.reject(FilesystemReadError::OutOfRange, failure)
			} else {
				self.budget
					.reject(FilesystemReadError::Source, error.into())
			}
		})
	}
}
