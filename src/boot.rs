use std::path::Path;

use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_foundation::NSString;
use objc2_virtualization::VZLinuxBootLoader;

use crate::util::path_to_nsurl;
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

    let loader = unsafe {
        VZLinuxBootLoader::initWithKernelURL(VZLinuxBootLoader::alloc(), &kernel_url)
    };

    unsafe { loader.setInitialRamdiskURL(Some(&initrd_url)) };
    unsafe { loader.setCommandLine(&cmdline) };

    Ok(loader)
}
