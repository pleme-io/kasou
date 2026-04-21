use std::path::PathBuf;

use objc2::AnyThread;
use objc2_foundation::NSString;
use objc2_virtualization::{
    VZEFIBootLoader, VZEFIVariableStore, VZEFIVariableStoreInitializationOptions,
    VZLinuxBootLoader, VZVirtualMachineConfiguration,
};

use crate::util::path_to_nsurl;
use crate::KasouError;

/// Boot loader configuration.
///
/// Two variants are supported today:
///
/// - `Linux` — direct Linux kernel boot. The kernel ELF and initrd are loaded
///   by the framework; `cmdline` is passed to the kernel. Matches the
///   traditional `VZLinuxBootLoader` flow.
///
/// - `Efi` — UEFI firmware boot. The VM boots the first EFI-bootable image
///   it finds on any attached disk. Required for guests that ship as EFI
///   applications (e.g. [`brasa`](https://github.com/pleme-io/brasa)). If a
///   `variable_store` path is provided, EFI variables persist across boots.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BootConfig {
    /// Direct Linux kernel boot.
    Linux {
        /// Path to the Linux kernel image.
        kernel: PathBuf,
        /// Path to the initial ramdisk (initrd).
        initrd: PathBuf,
        /// Kernel command line arguments.
        cmdline: String,
    },
    /// UEFI firmware boot — boots from an EFI-bootable disk in the VM's disk set.
    Efi {
        /// Optional path to persist EFI variables across boots. If `None`,
        /// variables are volatile (fine for many cases; not for Secure Boot).
        /// The file is created if missing; reused if present.
        variable_store: Option<PathBuf>,
    },
}

impl BootConfig {
    /// Convenience: construct a Linux-boot config.
    pub fn linux(kernel: PathBuf, initrd: PathBuf, cmdline: impl Into<String>) -> Self {
        Self::Linux {
            kernel,
            initrd,
            cmdline: cmdline.into(),
        }
    }

    /// Convenience: construct an EFI-boot config with no persisted variables.
    pub fn efi() -> Self {
        Self::Efi {
            variable_store: None,
        }
    }

    /// Convenience: construct an EFI-boot config persisting variables to `path`.
    pub fn efi_with_variable_store(path: PathBuf) -> Self {
        Self::Efi {
            variable_store: Some(path),
        }
    }
}

/// Set the boot loader on a `VZVirtualMachineConfiguration`, dispatching on
/// the `BootConfig` variant. File existence for Linux-boot inputs is checked
/// here; for EFI, boot-disk existence is validated as part of the disk list.
pub(crate) fn setup_boot_loader(
    config: &BootConfig,
    vz_config: &VZVirtualMachineConfiguration,
) -> Result<(), KasouError> {
    match config {
        BootConfig::Linux {
            kernel,
            initrd,
            cmdline,
        } => {
            if !kernel.exists() {
                return Err(KasouError::BootFilesNotFound(format!(
                    "kernel not found: {}",
                    kernel.display()
                )));
            }
            if !initrd.exists() {
                return Err(KasouError::BootFilesNotFound(format!(
                    "initrd not found: {}",
                    initrd.display()
                )));
            }

            let kernel_url = path_to_nsurl(kernel)?;
            let initrd_url = path_to_nsurl(initrd)?;
            let cmdline_ns = NSString::from_str(cmdline);

            // SAFETY: alloc + initWithKernelURL is the documented init path
            // for VZLinuxBootLoader; the URL remains valid for the call.
            let loader = unsafe {
                VZLinuxBootLoader::initWithKernelURL(VZLinuxBootLoader::alloc(), &kernel_url)
            };
            // SAFETY: setInitialRamdiskURL / setCommandLine are documented
            // methods on VZLinuxBootLoader.
            unsafe {
                loader.setInitialRamdiskURL(Some(&initrd_url));
                loader.setCommandLine(&cmdline_ns);
            }

            // SAFETY: setBootLoader accepts any VZBootLoader subclass; the
            // auto-deref chain Retained<VZLinuxBootLoader> → &VZBootLoader
            // makes this sound.
            unsafe { vz_config.setBootLoader(Some(&loader)) };
        }
        BootConfig::Efi { variable_store } => {
            // SAFETY: new() is the documented init path for VZEFIBootLoader.
            let loader = unsafe { VZEFIBootLoader::new() };

            if let Some(store_path) = variable_store {
                let store_url = path_to_nsurl(store_path)?;
                let store = if store_path.exists() {
                    // SAFETY: initWithURL takes ownership of the NSURL for
                    // the duration of the call; VZEFIVariableStore retains
                    // the path internally.
                    unsafe {
                        VZEFIVariableStore::initWithURL(VZEFIVariableStore::alloc(), &store_url)
                    }
                } else {
                    // SAFETY: createVariableStoreAtURL writes to the
                    // filesystem and returns NSError on failure, which we
                    // propagate as KasouError::Framework.
                    unsafe {
                        VZEFIVariableStore::initCreatingVariableStoreAtURL_options_error(
                            VZEFIVariableStore::alloc(),
                            &store_url,
                            VZEFIVariableStoreInitializationOptions::empty(),
                        )
                    }
                    .map_err(|e| {
                        KasouError::Framework(format!(
                            "failed to create EFI variable store at {}: {}",
                            store_path.display(),
                            e.localizedDescription()
                        ))
                    })?
                };
                // SAFETY: setVariableStore accepts Option<&VZEFIVariableStore>.
                unsafe { loader.setVariableStore(Some(&store)) };
            }

            // SAFETY: same reasoning as the Linux arm — deref to
            // &VZBootLoader is sound.
            unsafe { vz_config.setBootLoader(Some(&loader)) };
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_constructor() {
        let config = BootConfig::linux(
            PathBuf::from("/k"),
            PathBuf::from("/i"),
            "console=hvc0",
        );
        match config {
            BootConfig::Linux { kernel, initrd, cmdline } => {
                assert_eq!(kernel, PathBuf::from("/k"));
                assert_eq!(initrd, PathBuf::from("/i"));
                assert_eq!(cmdline, "console=hvc0");
            }
            BootConfig::Efi { .. } => panic!("wrong variant"),
        }
    }

    #[test]
    fn efi_constructor_volatile() {
        match BootConfig::efi() {
            BootConfig::Efi { variable_store: None } => {}
            _ => panic!("expected Efi with no variable store"),
        }
    }

    #[test]
    fn efi_constructor_with_store() {
        match BootConfig::efi_with_variable_store(PathBuf::from("/tmp/efi.vars")) {
            BootConfig::Efi { variable_store: Some(p) } => {
                assert_eq!(p, PathBuf::from("/tmp/efi.vars"));
            }
            _ => panic!("expected Efi with variable store"),
        }
    }
}
