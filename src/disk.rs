use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_virtualization::{
    VZDiskImageCachingMode, VZDiskImageStorageDeviceAttachment,
    VZDiskImageSynchronizationMode, VZVirtioBlockDeviceConfiguration,
};

use crate::util::{ns_error_description, path_to_nsurl};
use crate::KasouError;

/// Configuration for a virtio block device backed by a disk image.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DiskConfig {
    /// Path to the raw disk image file.
    pub path: std::path::PathBuf,
    /// Whether the disk should be mounted read-only.
    pub read_only: bool,
}

pub(crate) fn create_storage_device(
    config: &DiskConfig,
) -> Result<Retained<VZVirtioBlockDeviceConfiguration>, KasouError> {
    if !config.path.exists() {
        return Err(KasouError::DiskNotFound(config.path.clone()));
    }

    let url = path_to_nsurl(&config.path)?;

    // Use explicit cached + full sync to prevent EXT4 filesystem corruption
    // in Linux guests. The default .automatic mode is known to corrupt ext4.
    // See: https://github.com/utmapp/UTM/pull/5919
    let attachment = unsafe {
        VZDiskImageStorageDeviceAttachment::initWithURL_readOnly_cachingMode_synchronizationMode_error(
            VZDiskImageStorageDeviceAttachment::alloc(),
            &url,
            config.read_only,
            VZDiskImageCachingMode::Cached,
            VZDiskImageSynchronizationMode::Full,
        )
    }
    .map_err(|e| {
        KasouError::Framework(format!(
            "failed to create disk attachment for {}: {}",
            config.path.display(),
            ns_error_description(&e)
        ))
    })?;

    let device = unsafe {
        VZVirtioBlockDeviceConfiguration::initWithAttachment(
            VZVirtioBlockDeviceConfiguration::alloc(),
            &attachment,
        )
    };

    Ok(device)
}
