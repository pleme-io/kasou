use std::fmt;
use std::path::PathBuf;

use objc2_foundation::NSError;

/// Structured projection of an Apple `NSError`.
///
/// Apple's NSError carries six independent fields that explain a
/// failure: `domain`, `code`, `localizedDescription`, an optional
/// `localizedFailureReason`, an optional `localizedRecoverySuggestion`,
/// and a chain of `underlyingErrors`. Until this primitive landed,
/// kasou wrapped only `localizedDescription` and dropped the rest —
/// every debugging session that touched a VZ error class had to
/// reverse-engineer from one English sentence what Apple had given
/// us at five other keys.
///
/// `FrameworkError` captures all of them; `Display` emits a single
/// human-readable line per layer, with `\n  caused by:` indentation
/// for `underlying` entries. Consumers that need machine-readable
/// structure (status surfaces, telemetry) get it via `serde`.
///
/// Construction goes through [`crate::util::ns_error_chain`] which
/// walks the NSError tree once at the boundary; downstream code
/// works against the typed struct.
///
/// 2026-05-20 incident: the kasou-wrapped VZ error
/// `"VM operation failed: start failed: Invalid virtual machine
/// configuration. The storage device attachment is invalid."` was
/// `localizedDescription` only. Apple's domain/code + any
/// `failureReason` were nowhere in the log. With this type the
/// classifier sees the whole story.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FrameworkError {
    /// `NSError.domain` — Apple's category (e.g. `"VZErrorDomain"`).
    pub domain: String,
    /// `NSError.code` — integer specific to the domain.
    pub code: i64,
    /// `NSError.localizedDescription` — the one English sentence
    /// every consumer was previously stuck with. Always present.
    pub localized_description: String,
    /// `NSError.localizedFailureReason` — often the specific cause
    /// (`"Bad block alignment"`, `"File already attached"`, …).
    pub failure_reason: Option<String>,
    /// `NSError.localizedRecoverySuggestion` — operator hint when
    /// Apple supplies one.
    pub recovery_suggestion: Option<String>,
    /// `NSError.underlyingErrors` — the cause chain. Empty when
    /// Apple didn't supply one (the common case for VZ).
    pub underlying: Vec<FrameworkError>,
}

impl FrameworkError {
    /// Construct a `FrameworkError` by walking an `NSError` once.
    ///
    /// Safety: caller must own a live reference to `error` —
    /// `&NSError` already enforces this at the type level.
    #[must_use]
    pub fn from_ns_error(error: &NSError) -> Self {
        Self {
            domain: error.domain().to_string(),
            #[allow(clippy::cast_possible_truncation)]
            code: error.code() as i64,
            localized_description: error.localizedDescription().to_string(),
            failure_reason: error.localizedFailureReason().map(|s| s.to_string()),
            recovery_suggestion: error.localizedRecoverySuggestion().map(|s| s.to_string()),
            underlying: error
                .underlyingErrors()
                .iter()
                .map(|e| Self::from_ns_error(&e))
                .collect(),
        }
    }
}

impl fmt::Display for FrameworkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.localized_description)?;
        if let Some(reason) = &self.failure_reason {
            write!(f, " (reason: {reason})")?;
        }
        if let Some(sugg) = &self.recovery_suggestion {
            write!(f, " (suggestion: {sugg})")?;
        }
        write!(f, " [{}:{}]", self.domain, self.code)?;
        for u in &self.underlying {
            write!(f, "\n  caused by: {u}")?;
        }
        Ok(())
    }
}

/// Errors that can occur during VM configuration and lifecycle management.
#[derive(Debug, thiserror::Error)]
pub enum KasouError {
    #[error("invalid VM configuration: {0}")]
    InvalidConfig(String),

    #[error("VM operation failed: {0}")]
    OperationFailed(String),

    #[error("invalid MAC address: {0}")]
    InvalidMac(String),

    #[error("disk image not found: {}", .0.display())]
    DiskNotFound(PathBuf),

    #[error("boot files not found: {0}")]
    BootFilesNotFound(String),

    #[error("Virtualization.framework error: {0}")]
    Framework(String),

    #[error("configuration validation failed: {0}")]
    Validation(String),

    #[error("VM is in state {current}, expected {expected}")]
    InvalidState {
        current: crate::vm::VmState,
        expected: &'static str,
    },

    #[error("dispatch queue operation cancelled")]
    QueueCancelled,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Single-layer `FrameworkError` Display: emits localized_description,
    /// then [domain:code]. No leading "caused by:" indentation when
    /// `underlying` is empty (the common VZ case).
    #[test]
    fn display_single_layer_no_underlying() {
        let e = FrameworkError {
            domain: "VZErrorDomain".into(),
            code: 7,
            localized_description: "Invalid virtual machine configuration.".into(),
            failure_reason: None,
            recovery_suggestion: None,
            underlying: vec![],
        };
        let s = format!("{e}");
        assert_eq!(
            s,
            "Invalid virtual machine configuration. [VZErrorDomain:7]",
            "got: {s}"
        );
    }

    /// Failure reason + recovery suggestion both appear in Display
    /// when Apple provides them.
    #[test]
    fn display_includes_reason_and_suggestion() {
        let e = FrameworkError {
            domain: "VZErrorDomain".into(),
            code: 7,
            localized_description: "Invalid virtual machine configuration.".into(),
            failure_reason: Some("Storage attachment shape rejected.".into()),
            recovery_suggestion: Some("Re-check disk paths.".into()),
            underlying: vec![],
        };
        let s = format!("{e}");
        assert!(s.contains("(reason: Storage attachment shape rejected.)"), "got: {s}");
        assert!(s.contains("(suggestion: Re-check disk paths.)"), "got: {s}");
        assert!(s.contains("[VZErrorDomain:7]"), "got: {s}");
    }

    /// Underlying chain renders with `\n  caused by: ...` indentation
    /// per level. Confirms recursive Display.
    #[test]
    fn display_walks_underlying_chain() {
        let inner = FrameworkError {
            domain: "POSIXErrorDomain".into(),
            code: 13,
            localized_description: "Permission denied.".into(),
            failure_reason: None,
            recovery_suggestion: None,
            underlying: vec![],
        };
        let outer = FrameworkError {
            domain: "VZErrorDomain".into(),
            code: 7,
            localized_description: "Failed to open storage.".into(),
            failure_reason: None,
            recovery_suggestion: None,
            underlying: vec![inner],
        };
        let s = format!("{outer}");
        assert!(s.contains("Failed to open storage."), "got: {s}");
        assert!(s.contains("\n  caused by: Permission denied."), "got: {s}");
        assert!(s.contains("[POSIXErrorDomain:13]"), "got: {s}");
    }

    /// `Clone` + `PartialEq` + `Eq` derive correctness — operators
    /// that pipe kasou errors through tameshi attestation, status
    /// snapshots, or telemetry depend on these. (JSON shape is
    /// covered downstream where `serde_json` is already a dep —
    /// kasou itself stays leaf-thin.)
    #[test]
    fn clone_and_equality_via_derive() {
        let e = FrameworkError {
            domain: "VZErrorDomain".into(),
            code: 7,
            localized_description: "x".into(),
            failure_reason: Some("y".into()),
            recovery_suggestion: None,
            underlying: vec![FrameworkError {
                domain: "POSIXErrorDomain".into(),
                code: 13,
                localized_description: "Permission denied.".into(),
                failure_reason: None,
                recovery_suggestion: None,
                underlying: vec![],
            }],
        };
        let cloned = e.clone();
        assert_eq!(cloned, e);
        let mut tampered = e.clone();
        tampered.code = 99;
        assert_ne!(tampered, e);
    }
}
