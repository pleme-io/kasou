use std::path::PathBuf;

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

    #[error("VM is not in a valid state for this operation: current={current}, expected={expected}")]
    InvalidState {
        current: String,
        expected: String,
    },

    #[error("dispatch queue operation cancelled")]
    QueueCancelled,
}
