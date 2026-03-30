//! Shared utilities for Objective-C / Foundation interop.

use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_foundation::{NSError, NSString, NSURL};

use crate::KasouError;

/// Convert a filesystem path to an NSURL.
pub(crate) fn path_to_nsurl(path: &std::path::Path) -> Result<Retained<NSURL>, KasouError> {
    let path_str = path.to_str().ok_or_else(|| {
        KasouError::InvalidConfig(format!("path is not valid UTF-8: {}", path.display()))
    })?;
    let ns_path = NSString::from_str(path_str);
    Ok(NSURL::initFileURLWithPath(NSURL::alloc(), &ns_path))
}

/// Extract a human-readable description from an NSError.
pub(crate) fn ns_error_description(error: &NSError) -> String {
    error.localizedDescription().to_string()
}
