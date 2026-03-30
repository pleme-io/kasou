use std::path::Path;

use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_foundation::{NSString, NSURL};
use objc2_virtualization::{
    VZSharedDirectory, VZSingleDirectoryShare, VZVirtioFileSystemDeviceConfiguration,
};

use crate::KasouError;

/// Configuration for sharing a host directory with the guest via virtiofs.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SharedDirConfig {
    /// Tag used by the guest to mount this share (e.g. "share0").
    pub tag: String,
    /// Path on the host to share.
    pub host_path: std::path::PathBuf,
    /// Whether the share is read-only.
    pub read_only: bool,
}

pub(crate) fn create_shared_dir(
    config: &SharedDirConfig,
) -> Result<Retained<VZVirtioFileSystemDeviceConfiguration>, KasouError> {
    if !config.host_path.exists() {
        return Err(KasouError::InvalidConfig(format!(
            "shared directory does not exist: {}",
            config.host_path.display()
        )));
    }

    let url = path_to_nsurl(&config.host_path)?;
    let tag = NSString::from_str(&config.tag);

    // SAFETY: Creates a shared directory pointing to a valid host path.
    let shared_dir = unsafe {
        VZSharedDirectory::initWithURL_readOnly(
            VZSharedDirectory::alloc(),
            &url,
            config.read_only,
        )
    };

    // SAFETY: Wraps the shared directory in a single-directory share.
    let share = unsafe {
        VZSingleDirectoryShare::initWithDirectory(VZSingleDirectoryShare::alloc(), &shared_dir)
    };

    // SAFETY: Creates a virtio filesystem device with the given tag.
    let device = unsafe {
        VZVirtioFileSystemDeviceConfiguration::initWithTag(
            VZVirtioFileSystemDeviceConfiguration::alloc(),
            &tag,
        )
    };

    // SAFETY: setShare is valid on any VZVirtioFileSystemDeviceConfiguration.
    unsafe { device.setShare(Some(&share)) };

    Ok(device)
}

fn path_to_nsurl(path: &Path) -> Result<Retained<NSURL>, KasouError> {
    let path_str = path.to_str().ok_or_else(|| {
        KasouError::InvalidConfig(format!("path is not valid UTF-8: {}", path.display()))
    })?;
    let ns_path = NSString::from_str(path_str);
    // SAFETY: initFileURLWithPath creates a file URL from a filesystem path.
    let url = NSURL::initFileURLWithPath(NSURL::alloc(), &ns_path);
    Ok(url)
}
