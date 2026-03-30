use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_foundation::NSString;
use objc2_virtualization::{
    VZSharedDirectory, VZSingleDirectoryShare, VZVirtioFileSystemDeviceConfiguration,
};

use crate::util::path_to_nsurl;
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

    let shared_dir = unsafe {
        VZSharedDirectory::initWithURL_readOnly(VZSharedDirectory::alloc(), &url, config.read_only)
    };

    let share = unsafe {
        VZSingleDirectoryShare::initWithDirectory(VZSingleDirectoryShare::alloc(), &shared_dir)
    };

    let device = unsafe {
        VZVirtioFileSystemDeviceConfiguration::initWithTag(
            VZVirtioFileSystemDeviceConfiguration::alloc(),
            &tag,
        )
    };

    unsafe { device.setShare(Some(&share)) };

    Ok(device)
}
