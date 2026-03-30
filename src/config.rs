use objc2::rc::Retained;
use objc2_foundation::NSArray;
use objc2_virtualization::{
    VZDirectorySharingDeviceConfiguration, VZEntropyDeviceConfiguration,
    VZGenericPlatformConfiguration, VZNetworkDeviceConfiguration,
    VZSerialPortConfiguration, VZStorageDeviceConfiguration,
    VZVirtioEntropyDeviceConfiguration, VZVirtualMachineConfiguration,
};

use crate::boot::{self, BootConfig};
use crate::disk::{self, DiskConfig};
use crate::network::{self, NetworkConfig};
use crate::serial::{self, SerialConfig};
use crate::shared_dir::{self, SharedDirConfig};
use crate::KasouError;

/// Complete VM configuration.
///
/// Mirrors the parameters that vfkit accepts on its command line,
/// but with full MAC address control at the hypervisor level.
#[derive(Debug, Clone)]
pub struct VmConfig {
    /// Number of virtual CPUs.
    pub cpus: u32,
    /// Memory size in MiB.
    pub memory_mib: u64,
    /// Linux direct boot configuration (kernel, initrd, cmdline).
    pub boot: BootConfig,
    /// Disk images to attach as virtio block devices.
    pub disks: Vec<DiskConfig>,
    /// Network configuration (NAT + MAC address).
    pub network: NetworkConfig,
    /// Optional serial console log output.
    pub serial: Option<SerialConfig>,
    /// Host directories to share with the guest via virtiofs.
    pub shared_dirs: Vec<SharedDirConfig>,
}

impl VmConfig {
    /// Validate the configuration without building VZ objects.
    pub fn validate(&self) -> Result<(), KasouError> {
        if self.cpus == 0 {
            return Err(KasouError::Validation("cpus must be >= 1".into()));
        }
        if self.memory_mib == 0 {
            return Err(KasouError::Validation("memory_mib must be >= 1".into()));
        }
        if self.disks.is_empty() {
            return Err(KasouError::Validation("at least one disk is required".into()));
        }
        if !self.boot.kernel.exists() {
            return Err(KasouError::BootFilesNotFound(format!(
                "kernel: {}",
                self.boot.kernel.display()
            )));
        }
        if !self.boot.initrd.exists() {
            return Err(KasouError::BootFilesNotFound(format!(
                "initrd: {}",
                self.boot.initrd.display()
            )));
        }
        for disk in &self.disks {
            if !disk.path.exists() {
                return Err(KasouError::DiskNotFound(disk.path.clone()));
            }
        }
        Ok(())
    }
}

/// Build a `VZVirtualMachineConfiguration` from our `VmConfig`.
///
/// Wrapped in `objc2::exception::catch` to handle NSExceptions from the
/// framework (e.g., invalid parameters that throw rather than return NSError).
pub(crate) fn build_vz_config(
    config: &VmConfig,
) -> Result<Retained<VZVirtualMachineConfiguration>, KasouError> {
    // Catch ObjC exceptions that VZ may throw for deeply invalid configs.
    // These would otherwise SIGTRAP the process.
    let result = unsafe {
        objc2::exception::catch(|| build_vz_config_inner(config))
    };
    match result {
        Ok(inner) => inner,
        Err(exception) => {
            let desc = format!("{exception:?}");
            Err(KasouError::Framework(format!(
                "Objective-C exception during VZ configuration: {desc}"
            )))
        }
    }
}

fn build_vz_config_inner(
    config: &VmConfig,
) -> Result<Retained<VZVirtualMachineConfiguration>, KasouError> {
    let vz_config = unsafe { VZVirtualMachineConfiguration::new() };

    // CPU and memory
    // SAFETY: setCPUCount and setMemorySize are valid on any VZVirtualMachineConfiguration.
    unsafe {
        vz_config.setCPUCount(config.cpus as usize);
        vz_config.setMemorySize(config.memory_mib * 1024 * 1024);
    }

    // Platform: generic (Linux)
    // SAFETY: VZGenericPlatformConfiguration::new() creates a default platform.
    let platform = unsafe { VZGenericPlatformConfiguration::new() };
    // SAFETY: setPlatform is valid with any VZPlatformConfiguration subclass.
    unsafe { vz_config.setPlatform(&platform) };

    // Boot loader
    let boot_loader = boot::create_boot_loader(&config.boot)?;
    // SAFETY: setBootLoader is valid with any VZBootLoader subclass.
    unsafe { vz_config.setBootLoader(Some(&boot_loader)) };

    // Storage devices — collect as superclass refs for NSArray type compatibility
    let mut storage_devices = Vec::new();
    for disk in &config.disks {
        let device = disk::create_storage_device(disk)?;
        storage_devices.push(device);
    }
    let storage_refs: Vec<&VZStorageDeviceConfiguration> =
        storage_devices.iter().map(|d| -> &VZStorageDeviceConfiguration { d }).collect();
    let storage_array = NSArray::from_slice(&storage_refs);
    // SAFETY: setStorageDevices is valid with an NSArray of VZStorageDeviceConfiguration.
    unsafe { vz_config.setStorageDevices(&storage_array) };

    // Network device — upcast to superclass ref
    let network_device = network::create_network_device(&config.network)?;
    let net_ref: &VZNetworkDeviceConfiguration = &network_device;
    let network_array = NSArray::from_slice(&[net_ref]);
    unsafe { vz_config.setNetworkDevices(&network_array) };

    // Serial port (optional)
    if let Some(serial_config) = &config.serial {
        let serial_port = serial::create_serial_port(serial_config)?;
        let serial_ref: &VZSerialPortConfiguration = &serial_port;
        let serial_array = NSArray::from_slice(&[serial_ref]);
        // SAFETY: setSerialPorts is valid with an NSArray of VZSerialPortConfiguration.
        unsafe { vz_config.setSerialPorts(&serial_array) };
    }

    // Shared directories (optional)
    if !config.shared_dirs.is_empty() {
        let mut dir_devices = Vec::new();
        for dir_config in &config.shared_dirs {
            let device = shared_dir::create_shared_dir(dir_config)?;
            dir_devices.push(device);
        }
        let dir_refs: Vec<&VZDirectorySharingDeviceConfiguration> =
            dir_devices.iter().map(|d| -> &VZDirectorySharingDeviceConfiguration { d }).collect();
        let dir_array = NSArray::from_slice(&dir_refs);
        // SAFETY: setDirectorySharingDevices is valid.
        unsafe { vz_config.setDirectorySharingDevices(&dir_array) };
    }

    // Entropy device (provides /dev/random to guest)
    // SAFETY: VZVirtioEntropyDeviceConfiguration::new() creates a default entropy device.
    let entropy = unsafe { VZVirtioEntropyDeviceConfiguration::new() };
    let entropy_ref: &VZEntropyDeviceConfiguration = &entropy;
    let entropy_array = NSArray::from_slice(&[entropy_ref]);
    // SAFETY: setEntropyDevices is valid.
    unsafe { vz_config.setEntropyDevices(&entropy_array) };

    tracing::debug!("validating VZ configuration...");

    // Validate the complete configuration
    // SAFETY: validateWithError checks all VZ constraints and returns an error if invalid.
    unsafe { vz_config.validateWithError() }.map_err(|e| {
        let desc = e.localizedDescription().to_string();
        KasouError::Framework(format!("VM configuration validation failed: {desc}"))
    })?;

    tracing::debug!("VZ configuration validated successfully");
    Ok(vz_config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn validate_rejects_zero_cpus() {
        let config = VmConfig {
            cpus: 0,
            memory_mib: 1024,
            boot: BootConfig {
                kernel: PathBuf::from("/nonexistent/kernel"),
                initrd: PathBuf::from("/nonexistent/initrd"),
                cmdline: String::new(),
            },
            disks: vec![DiskConfig {
                path: PathBuf::from("/nonexistent/disk"),
                read_only: false,
            }],
            network: NetworkConfig { mac_address: None },
            serial: None,
            shared_dirs: vec![],
        };
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("cpus"));
    }

    #[test]
    fn validate_rejects_zero_memory() {
        let config = VmConfig {
            cpus: 1,
            memory_mib: 0,
            boot: BootConfig {
                kernel: PathBuf::from("/nonexistent/kernel"),
                initrd: PathBuf::from("/nonexistent/initrd"),
                cmdline: String::new(),
            },
            disks: vec![DiskConfig {
                path: PathBuf::from("/nonexistent/disk"),
                read_only: false,
            }],
            network: NetworkConfig { mac_address: None },
            serial: None,
            shared_dirs: vec![],
        };
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("memory"));
    }

    #[test]
    fn validate_rejects_no_disks() {
        let config = VmConfig {
            cpus: 1,
            memory_mib: 1024,
            boot: BootConfig {
                kernel: PathBuf::from("/nonexistent/kernel"),
                initrd: PathBuf::from("/nonexistent/initrd"),
                cmdline: String::new(),
            },
            disks: vec![],
            network: NetworkConfig { mac_address: None },
            serial: None,
            shared_dirs: vec![],
        };
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("disk"));
    }
}
