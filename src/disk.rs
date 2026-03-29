use std::path::Path;

use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_foundation::{NSString, NSURL};
use objc2_virtualization::{VZDiskImageStorageDeviceAttachment, VZVirtioBlockDeviceConfiguration};

use crate::KasouError;

/// Configuration for a virtio block device backed by a disk image.
#[derive(Debug, Clone)]
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

    // SAFETY: Creates a disk image attachment from a file URL.
    // Returns an error via NSError if the file cannot be opened.
    let attachment = unsafe {
        VZDiskImageStorageDeviceAttachment::initWithURL_readOnly_error(
            VZDiskImageStorageDeviceAttachment::alloc(),
            &url,
            config.read_only,
        )
    }
    .map_err(|e| {
        KasouError::Framework(format!(
            "failed to create disk attachment for {}: {}",
            config.path.display(),
            ns_error_description(&e)
        ))
    })?;

    // SAFETY: Creates a virtio block device from a valid storage attachment.
    let device = unsafe {
        VZVirtioBlockDeviceConfiguration::initWithAttachment(
            VZVirtioBlockDeviceConfiguration::alloc(),
            &attachment,
        )
    };

    Ok(device)
}

fn path_to_nsurl(path: &Path) -> Result<Retained<NSURL>, KasouError> {
    let path_str = path.to_str().ok_or_else(|| {
        KasouError::InvalidConfig(format!("path is not valid UTF-8: {}", path.display()))
    })?;
    let ns_path = NSString::from_str(path_str);

    // SAFETY: initFileURLWithPath creates a file URL from a filesystem path string.
    let url = NSURL::initFileURLWithPath(NSURL::alloc(), &ns_path);
    Ok(url)
}

fn ns_error_description(error: &objc2_foundation::NSError) -> String {
    error.localizedDescription().to_string()
}
