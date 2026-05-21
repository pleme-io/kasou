//! Shared utilities for Objective-C / Foundation interop.

use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_foundation::{NSError, NSString, NSURL};

use crate::error::FrameworkError;
use crate::KasouError;

/// Convert a filesystem path to an NSURL.
pub(crate) fn path_to_nsurl(path: &std::path::Path) -> Result<Retained<NSURL>, KasouError> {
    let path_str = path.to_str().ok_or_else(|| {
        KasouError::InvalidConfig(format!("path is not valid UTF-8: {}", path.display()))
    })?;
    let ns_path = NSString::from_str(path_str);
    Ok(NSURL::initFileURLWithPath(NSURL::alloc(), &ns_path))
}

/// Walk an NSError and capture its full cause chain into a typed
/// `FrameworkError`. Replaces the old `ns_error_description` which
/// returned only `localizedDescription` and discarded `failureReason`,
/// `recoverySuggestion`, `domain`, `code`, and `underlyingErrors`.
///
/// Cheap — one pass over the NSError tree, the only NSError methods
/// that can fail are guarded inside `FrameworkError::from_ns_error`.
#[must_use]
pub(crate) fn ns_error_chain(error: &NSError) -> FrameworkError {
    FrameworkError::from_ns_error(error)
}
