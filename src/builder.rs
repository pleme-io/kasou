//! Type-safe builder for VM configuration.
//!
//! ```
//! use kasou::VmConfigBuilder;
//! use std::path::PathBuf;
//!
//! # fn example() -> Result<(), kasou::KasouError> {
//! let config = VmConfigBuilder::new("my-vm")
//!     .cpus(4)
//!     .memory_mib(8192)
//!     .boot(PathBuf::from("/path/to/kernel"))
//!     .initrd(PathBuf::from("/path/to/initrd"))
//!     .cmdline("console=hvc0 root=/dev/vda")
//!     .disk(PathBuf::from("/path/to/root.img"))
//!     .disk_readonly(PathBuf::from("/path/to/seed.img"))
//!     .nat_network()
//!     .mac("5a:94:ef:ab:cd:12")
//!     .serial_file(PathBuf::from("/tmp/console.log"))
//!     .build()?;
//! # Ok(())
//! # }
//! ```

use std::path::PathBuf;

use crate::boot::BootConfig;
use crate::config::VmConfig;
use crate::disk::DiskConfig;
use crate::network::NetworkConfig;
use crate::serial::SerialConfig;
use crate::shared_dir::SharedDirConfig;
use crate::types::{MacAddress, VmId};
use crate::KasouError;

/// Internal builder mode — Linux direct boot (default) or EFI boot.
/// Callers flip to EFI via `.efi_boot()`.
#[derive(Debug, Clone)]
enum BootMode {
    /// Linux direct boot accumulator. Default.
    Linux {
        kernel: Option<PathBuf>,
        initrd: Option<PathBuf>,
        cmdline: Option<String>,
    },
    /// EFI firmware boot accumulator.
    Efi {
        variable_store: Option<PathBuf>,
    },
}

impl Default for BootMode {
    fn default() -> Self {
        Self::Linux {
            kernel: None,
            initrd: None,
            cmdline: None,
        }
    }
}

/// Builder for constructing `VmConfig` with a fluent API.
pub struct VmConfigBuilder {
    id: VmId,
    cpus: u32,
    memory_mib: u64,
    boot_mode: BootMode,
    disks: Vec<DiskConfig>,
    mac: Option<String>,
    serial: Option<SerialConfig>,
    shared_dirs: Vec<SharedDirConfig>,
}

impl VmConfigBuilder {
    /// Create a new builder with the given VM identifier.
    pub fn new(id: impl Into<VmId>) -> Self {
        Self {
            id: id.into(),
            cpus: 2,
            memory_mib: 2048,
            boot_mode: BootMode::default(),
            disks: Vec::new(),
            mac: None,
            serial: None,
            shared_dirs: Vec::new(),
        }
    }

    /// Set the number of virtual CPUs.
    pub fn cpus(mut self, count: u32) -> Self {
        self.cpus = count;
        self
    }

    /// Set memory size in MiB.
    pub fn memory_mib(mut self, size: u64) -> Self {
        self.memory_mib = size;
        self
    }

    /// Set the kernel path for direct Linux boot. Implicitly selects Linux
    /// boot mode; fails later at `build()` if combined with EFI-only fields.
    pub fn boot(mut self, kernel: PathBuf) -> Self {
        self.boot_mode = match self.boot_mode {
            BootMode::Linux { initrd, cmdline, .. } => BootMode::Linux {
                kernel: Some(kernel),
                initrd,
                cmdline,
            },
            BootMode::Efi { .. } => BootMode::Linux {
                kernel: Some(kernel),
                initrd: None,
                cmdline: None,
            },
        };
        self
    }

    /// Set the initrd path (order-independent — can be called before or after `boot()`).
    pub fn initrd(mut self, initrd: PathBuf) -> Self {
        self.boot_mode = match self.boot_mode {
            BootMode::Linux { kernel, cmdline, .. } => BootMode::Linux {
                kernel,
                initrd: Some(initrd),
                cmdline,
            },
            BootMode::Efi { .. } => BootMode::Linux {
                kernel: None,
                initrd: Some(initrd),
                cmdline: None,
            },
        };
        self
    }

    /// Set the kernel command line (order-independent).
    pub fn cmdline(mut self, cmdline: impl Into<String>) -> Self {
        self.boot_mode = match self.boot_mode {
            BootMode::Linux { kernel, initrd, .. } => BootMode::Linux {
                kernel,
                initrd,
                cmdline: Some(cmdline.into()),
            },
            BootMode::Efi { .. } => BootMode::Linux {
                kernel: None,
                initrd: None,
                cmdline: Some(cmdline.into()),
            },
        };
        self
    }

    /// Switch the builder into EFI boot mode. The boot disk is added via
    /// `.disk(…)` like any other disk; the EFI firmware scans the disk set
    /// and boots the first EFI-bootable image. No separate kernel/initrd is
    /// needed on the host side.
    ///
    /// Use `.efi_variable_store(…)` to persist EFI variables across boots.
    pub fn efi_boot(mut self) -> Self {
        self.boot_mode = match self.boot_mode {
            BootMode::Efi { variable_store } => BootMode::Efi { variable_store },
            BootMode::Linux { .. } => BootMode::Efi { variable_store: None },
        };
        self
    }

    /// Persist EFI variables (including Secure Boot state and boot-order
    /// preferences) to `path` across boots. Implies EFI boot mode.
    pub fn efi_variable_store(mut self, path: PathBuf) -> Self {
        self.boot_mode = BootMode::Efi {
            variable_store: Some(path),
        };
        self
    }

    /// Add a read-write disk.
    pub fn disk(mut self, path: PathBuf) -> Self {
        self.disks.push(DiskConfig {
            path,
            read_only: false,
        });
        self
    }

    /// Add a read-only disk.
    pub fn disk_readonly(mut self, path: PathBuf) -> Self {
        self.disks.push(DiskConfig {
            path,
            read_only: true,
        });
        self
    }

    /// Enable NAT networking.
    pub fn nat_network(self) -> Self {
        // NAT is the default, nothing to change
        self
    }

    /// Set the MAC address (colon-separated, e.g., "5a:94:ef:ab:cd:12").
    pub fn mac(mut self, mac: impl Into<String>) -> Self {
        self.mac = Some(mac.into());
        self
    }

    /// Generate a deterministic MAC from a seed (e.g., hostname) and the VM's ID.
    pub fn deterministic_mac(mut self, seed: &str) -> Self {
        self.mac = Some(MacAddress::deterministic(seed, &self.id.0).to_string());
        self
    }

    /// Set serial console output to a file.
    pub fn serial_file(mut self, path: PathBuf) -> Self {
        self.serial = Some(SerialConfig { log_path: path });
        self
    }

    /// Add a shared directory (virtiofs).
    pub fn shared_dir(
        mut self,
        tag: impl Into<String>,
        host_path: PathBuf,
        read_only: bool,
    ) -> Self {
        self.shared_dirs.push(SharedDirConfig {
            tag: tag.into(),
            host_path,
            read_only,
        });
        self
    }

    /// Build the `VmConfig`, validating all required fields.
    pub fn build(self) -> Result<VmConfig, KasouError> {
        let boot = match self.boot_mode {
            BootMode::Linux { kernel, initrd, cmdline } => {
                let kernel = kernel.ok_or_else(|| {
                    KasouError::Validation("kernel path is required (call .boot())".into())
                })?;
                let initrd = initrd.ok_or_else(|| {
                    KasouError::Validation("initrd path is required (call .initrd())".into())
                })?;
                BootConfig::Linux {
                    kernel,
                    initrd,
                    cmdline: cmdline.unwrap_or_default(),
                }
            }
            BootMode::Efi { variable_store } => BootConfig::Efi { variable_store },
        };

        let config = VmConfig {
            id: self.id,
            cpus: self.cpus,
            memory_mib: self.memory_mib,
            boot,
            disks: self.disks,
            network: NetworkConfig {
                mac_address: self.mac,
            },
            serial: self.serial,
            shared_dirs: self.shared_dirs,
        };

        config.validate()?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn builder_validates_missing_kernel() {
        let result = VmConfigBuilder::new("test")
            .initrd(PathBuf::from("/initrd"))
            .disk(PathBuf::from("/tmp/disk.img"))
            .build();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("kernel"));
    }

    #[test]
    fn builder_validates_missing_initrd() {
        let result = VmConfigBuilder::new("test")
            .boot(PathBuf::from("/kernel"))
            .disk(PathBuf::from("/tmp/disk.img"))
            .build();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("initrd"));
    }

    #[test]
    fn builder_order_independent() {
        // initrd and cmdline can be set before boot — no silent failure
        let builder = VmConfigBuilder::new("test")
            .cmdline("console=hvc0")
            .initrd(PathBuf::from("/initrd"))
            .boot(PathBuf::from("/kernel"))
            .disk(PathBuf::from("/disk.img"));

        match &builder.boot_mode {
            BootMode::Linux { kernel, initrd, cmdline } => {
                assert!(kernel.is_some());
                assert!(initrd.is_some());
                assert!(cmdline.is_some());
            }
            BootMode::Efi { .. } => panic!("expected Linux mode"),
        }
    }

    #[test]
    fn builder_efi_mode_no_kernel_required() {
        let config = VmConfigBuilder::new("brasa-test")
            .cpus(2)
            .memory_mib(1024)
            .efi_boot()
            .disk_readonly(PathBuf::from("/tmp/brasa.img"))
            .build();
        // Validation fails because the disk doesn't exist in the test env,
        // but it fails at the *disk* check, not the kernel check — proving
        // EFI mode doesn't require kernel/initrd.
        match config {
            Ok(_) => {}
            Err(e) => {
                let msg = e.to_string();
                assert!(
                    !msg.contains("kernel") && !msg.contains("initrd"),
                    "EFI mode should not require kernel/initrd, got: {msg}"
                );
            }
        }
    }

    #[test]
    fn builder_efi_variable_store_implies_efi_mode() {
        let builder = VmConfigBuilder::new("test")
            .efi_variable_store(PathBuf::from("/tmp/efi.vars"));
        match &builder.boot_mode {
            BootMode::Efi { variable_store: Some(p) } => {
                assert_eq!(p, &PathBuf::from("/tmp/efi.vars"));
            }
            _ => panic!("expected Efi mode with variable store"),
        }
    }

    #[test]
    fn builder_sets_deterministic_mac() {
        let builder = VmConfigBuilder::new("cid-k3s").deterministic_mac("my-host");
        assert!(builder.mac.is_some());
        assert!(builder.mac.unwrap().starts_with("52:55:55:"));
    }

    #[test]
    fn builder_fluent_api() {
        let builder = VmConfigBuilder::new("test")
            .cpus(4)
            .memory_mib(8192)
            .boot(PathBuf::from("/kernel"))
            .initrd(PathBuf::from("/initrd"))
            .cmdline("console=hvc0")
            .disk(PathBuf::from("/root.img"))
            .disk_readonly(PathBuf::from("/seed.img"))
            .nat_network()
            .mac("5a:94:ef:ab:cd:12")
            .serial_file(PathBuf::from("/console.log"))
            .shared_dir("share0", PathBuf::from("/tmp/share"), true);

        assert_eq!(builder.cpus, 4);
        assert_eq!(builder.memory_mib, 8192);
        assert_eq!(builder.disks.len(), 2);
        assert!(matches!(builder.boot_mode, BootMode::Linux { kernel: Some(_), .. }));
        assert!(builder.serial.is_some());
        assert_eq!(builder.shared_dirs.len(), 1);
    }
}
