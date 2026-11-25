//! Descriptor-relative checks for macOS extended access controls.

use std::{
	ffi::{c_int, c_void},
	io,
	os::fd::{AsRawFd, BorrowedFd},
	ptr::NonNull,
};

#[expect(
	unsafe_code,
	reason = "opaque descriptor ACL interface from the macOS SDK"
)]
unsafe extern "C" {
	fn acl_get_fd(fd: c_int) -> *mut c_void;
	fn acl_free(object: *mut c_void) -> c_int;
}

/// Checks whether an open file or directory lacks an extended access control list.
///
/// This checks the held object. Callers must also check its ownership and mode
/// before using it as a private directory or file.
///
/// # Errors
/// Returns an error if the descriptor's ACL cannot be queried or released.
#[expect(
	unsafe_code,
	reason = "held descriptor ACL query and release of its opaque result"
)]
pub fn no_extended_acl(fd: BorrowedFd<'_>) -> io::Result<bool> {
	// SAFETY: fd remains borrowed for the synchronous query. The function
	// returns either null or a caller-owned opaque ACL allocated by libc.
	let acl = unsafe { acl_get_fd(fd.as_raw_fd()) };
	if let Some(acl) = NonNull::new(acl) {
		// SAFETY: This non-null object came from acl_get_fd and is freed exactly
		// once with the designated C API. Its representation is never accessed.
		if unsafe { acl_free(acl.as_ptr()) } != 0 {
			return Err(io::Error::last_os_error());
		}
		Ok(false)
	} else {
		let error = io::Error::last_os_error();
		// Apple's acl_get_fd returns ENOENT when the held object lacks an ACL.
		if error.raw_os_error() == Some(rustix::io::Errno::NOENT.raw_os_error()) {
			Ok(true)
		} else {
			Err(error)
		}
	}
}
