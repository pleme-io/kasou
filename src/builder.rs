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

/// Builder for constructing `VmConfig` with a fluent API.
pub struct VmConfigBuilder {
    id: VmId,
    cpus: u32,
    memory_mib: u64,
    kernel: Option<PathBuf>,
    initrd: Option<PathBuf>,
    cmdline: Option<String>,
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
            kernel: None,
            initrd: None,
            cmdline: None,
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

    /// Set the kernel path for direct Linux boot.
    pub fn boot(mut self, kernel: PathBuf) -> Self {
        self.kernel = Some(kernel);
        self
    }

    /// Set the initrd path (order-independent — can be called before or after `boot()`).
    pub fn initrd(mut self, initrd: PathBuf) -> Self {
        self.initrd = Some(initrd);
        self
    }

    /// Set the kernel command line (order-independent).
    pub fn cmdline(mut self, cmdline: impl Into<String>) -> Self {
        self.cmdline = Some(cmdline.into());
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
        let kernel = self.kernel.ok_or_else(|| {
            KasouError::Validation("kernel path is required (call .boot())".into())
        })?;
        let initrd = self.initrd.ok_or_else(|| {
            KasouError::Validation("initrd path is required (call .initrd())".into())
        })?;
        let boot = BootConfig {
            kernel,
            initrd,
            cmdline: self.cmdline.unwrap_or_default(),
        };

        let config = VmConfig {
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

        assert!(builder.kernel.is_some());
        assert!(builder.initrd.is_some());
        assert!(builder.cmdline.is_some());
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
        assert!(builder.kernel.is_some());
        assert!(builder.serial.is_some());
        assert_eq!(builder.shared_dirs.len(), 1);
    }
}
