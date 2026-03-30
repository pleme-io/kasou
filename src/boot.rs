use std::path::Path;

use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_foundation::{NSString, NSURL};
use objc2_virtualization::VZLinuxBootLoader;

use crate::KasouError;

/// Configuration for direct Linux kernel boot.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BootConfig {
    /// Path to the Linux kernel image.
    pub kernel: std::path::PathBuf,
    /// Path to the initial ramdisk (initrd).
    pub initrd: std::path::PathBuf,
    /// Kernel command line arguments.
    pub cmdline: String,
}

pub(crate) fn create_boot_loader(config: &BootConfig) -> Result<Retained<VZLinuxBootLoader>, KasouError> {
    if !config.kernel.exists() {
        return Err(KasouError::BootFilesNotFound(format!(
            "kernel not found: {}",
            config.kernel.display()
        )));
    }
    if !config.initrd.exists() {
        return Err(KasouError::BootFilesNotFound(format!(
            "initrd not found: {}",
            config.initrd.display()
        )));
    }

    let kernel_url = path_to_nsurl(&config.kernel)?;
    let initrd_url = path_to_nsurl(&config.initrd)?;
    let cmdline = NSString::from_str(&config.cmdline);

    // SAFETY: initWithKernelURL takes a valid file URL for the kernel image.
    // We verified the file exists above.
    let loader = unsafe {
        VZLinuxBootLoader::initWithKernelURL(VZLinuxBootLoader::alloc(), &kernel_url)
    };

    // SAFETY: setInitialRamdiskURL is valid on any VZLinuxBootLoader instance.
    unsafe { loader.setInitialRamdiskURL(Some(&initrd_url)) };

    // SAFETY: setCommandLine is valid on any VZLinuxBootLoader instance.
    unsafe { loader.setCommandLine(&cmdline) };

    Ok(loader)
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
