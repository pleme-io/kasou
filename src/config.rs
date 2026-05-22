use objc2::rc::Retained;
use objc2_foundation::{NSArray, NSData};
use objc2_virtualization::{
    VZDirectorySharingDeviceConfiguration, VZEntropyDeviceConfiguration,
    VZGenericMachineIdentifier, VZGenericPlatformConfiguration,
    VZNetworkDeviceConfiguration, VZSerialPortConfiguration,
    VZStorageDeviceConfiguration, VZVirtioEntropyDeviceConfiguration,
    VZVirtualMachineConfiguration,
};

use crate::boot::{self, BootConfig};
// `boot::setup_boot_loader` dispatches on the `BootConfig` variant and sets
// the loader directly on `vz_config`; we no longer construct the loader here.
use crate::disk::{self, DiskConfig};
use crate::network::{self, NetworkConfig};
use crate::serial::{self, SerialConfig};
use crate::shared_dir::{self, SharedDirConfig};
use crate::KasouError;

/// Complete VM configuration.
///
/// Defines everything needed to create and boot a virtual machine:
/// CPU, memory, boot source, storage, networking, and peripherals.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VmConfig {
    /// Unique identifier for this VM.
    #[serde(default)]
    pub id: crate::types::VmId,
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

    /// Optional path to persist the typed
    /// `VZGenericMachineIdentifier` between runs. When `Some` and
    /// the file exists, the identifier is restored from its
    /// `dataRepresentation` bytes — required for snapshot-resume
    /// to succeed (VZ rejects restore with a mismatched identifier).
    /// When `Some` and the file is missing, a fresh identifier is
    /// generated + written to the path before VM creation. When
    /// `None`, the platform gets a process-lifetime identifier
    /// (no save/restore support).
    ///
    /// Recommended location: `<vm_dir>/machine-identifier.bin`.
    /// Persistence is fsync-anchored to survive crashes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine_identifier_path: Option<std::path::PathBuf>,
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
        match &self.boot {
            BootConfig::Linux { kernel, initrd, .. } => {
                if !kernel.exists() {
                    return Err(KasouError::BootFilesNotFound(format!(
                        "kernel: {}",
                        kernel.display()
                    )));
                }
                if !initrd.exists() {
                    return Err(KasouError::BootFilesNotFound(format!(
                        "initrd: {}",
                        initrd.display()
                    )));
                }
            }
            BootConfig::Efi { variable_store } => {
                // Variable-store presence is not a precondition — it's created
                // on first boot if missing. The EFI boot disk is just one of
                // `self.disks` and is validated in the disk loop below.
                if let Some(store) = variable_store {
                    // If a parent directory is specified, it must exist.
                    if let Some(parent) = store.parent() {
                        if !parent.as_os_str().is_empty() && !parent.exists() {
                            return Err(KasouError::Validation(format!(
                                "EFI variable store parent directory missing: {}",
                                parent.display()
                            )));
                        }
                    }
                }
            }
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
    let result = objc2::exception::catch(|| build_vz_config_inner(config));
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

    // Snapshot-resume fix — VZGenericMachineIdentifier persistence.
    //
    // VZ's restore-from-saved-state path rejects any VM whose
    // platform `machineIdentifier` differs from the one that was
    // active when the snapshot was taken. The previous code path
    // implicitly let VZ generate a fresh identifier on every boot
    // — so `save_state` worked but `restore_state` always failed
    // with `VZErrorDomain` code `VZErrorRestoreVirtualMachineFailed`.
    //
    // Fix: optionally persist the identifier's wire form
    // (`dataRepresentation`) to a typed path. On boot:
    //   * If the file exists, deserialize via
    //     `initWithDataRepresentation:` and reuse.
    //   * Otherwise, take the platform's fresh identifier +
    //     fsync-anchored write its bytes to the path.
    if let Some(id_path) = &config.machine_identifier_path {
        let identifier = load_or_create_machine_identifier(id_path, &platform)?;
        // SAFETY: setMachineIdentifier copies + retains internally.
        unsafe { platform.setMachineIdentifier(&identifier) };
    }
    // SAFETY: setPlatform is valid with any VZPlatformConfiguration subclass.
    unsafe { vz_config.setPlatform(&platform) };

    // Boot loader — dispatches on BootConfig variant (Linux vs EFI).
    boot::setup_boot_loader(&config.boot, &vz_config)?;

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
        let chain = crate::util::ns_error_chain(&e);
        KasouError::Framework(format!("VM configuration validation failed: {chain}"))
    })?;

    tracing::debug!("VZ configuration validated successfully");
    Ok(vz_config)
}

/// Load the typed machine identifier from `path` if it exists,
/// otherwise capture the freshly-generated identifier from
/// `platform` and persist its `dataRepresentation` bytes to
/// `path` (fsync-anchored).
///
/// The fsync is intentional: a crash mid-persist would otherwise
/// leave a zero-byte file that future restores can't deserialize,
/// permanently bricking snapshot-resume.
fn load_or_create_machine_identifier(
    path: &std::path::Path,
    platform: &VZGenericPlatformConfiguration,
) -> Result<Retained<VZGenericMachineIdentifier>, KasouError> {
    use std::io::Write;

    if path.exists() {
        let bytes = std::fs::read(path).map_err(|e| {
            KasouError::Validation(format!(
                "read machine identifier from {}: {e}",
                path.display()
            ))
        })?;
        // SAFETY: NSData::with_bytes copies the slice; the resulting
        // NSData lives long enough for initWithDataRepresentation.
        let nsdata = NSData::with_bytes(&bytes);
        use objc2::AllocAnyThread;
        let alloc = VZGenericMachineIdentifier::alloc();
        let maybe = unsafe {
            VZGenericMachineIdentifier::initWithDataRepresentation(alloc, &nsdata)
        };
        // initWithDataRepresentation: returns nil for malformed bytes.
        return maybe.ok_or_else(|| {
            KasouError::Validation(format!(
                "machine identifier at {} is corrupt or wrong format",
                path.display()
            ))
        });
    }

    // First boot: take the platform's auto-generated identifier
    // + persist its dataRepresentation atomically + fsync.
    // SAFETY: machineIdentifier returns the platform's current identifier.
    let identifier = unsafe { platform.machineIdentifier() };
    // SAFETY: dataRepresentation returns NSData bytes valid until drop.
    let data = unsafe { identifier.dataRepresentation() };
    let bytes_vec: Vec<u8> = data.to_vec();
    let bytes_slice: &[u8] = &bytes_vec;

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| {
                KasouError::Validation(format!(
                    "mkdir -p {}: {e}",
                    parent.display()
                ))
            })?;
        }
    }
    // Atomic write — temp file + rename + fsync the parent dir for
    // crash safety. Same pattern as save_state in vm.rs.
    let tmp = path.with_extension("bin.tmp");
    {
        let mut f = std::fs::File::create(&tmp).map_err(|e| {
            KasouError::Validation(format!("create {}: {e}", tmp.display()))
        })?;
        f.write_all(bytes_slice).map_err(|e| {
            KasouError::Validation(format!("write {}: {e}", tmp.display()))
        })?;
        f.sync_all().map_err(|e| {
            KasouError::Validation(format!("fsync {}: {e}", tmp.display()))
        })?;
    }
    std::fs::rename(&tmp, path).map_err(|e| {
        KasouError::Validation(format!(
            "rename {} → {}: {e}",
            tmp.display(),
            path.display()
        ))
    })?;
    Ok(identifier)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn validate_rejects_zero_cpus() {
        let config = VmConfig {
            id: crate::types::VmId::default(),
            cpus: 0,
            memory_mib: 1024,
            boot: BootConfig::Linux {
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
            machine_identifier_path: None,
        };
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("cpus"));
    }

    #[test]
    fn validate_rejects_zero_memory() {
        let config = VmConfig {
            id: crate::types::VmId::default(),
            cpus: 1,
            memory_mib: 0,
            boot: BootConfig::Linux {
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
            machine_identifier_path: None,
        };
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("memory"));
    }

    #[test]
    fn machine_identifier_round_trip() {
        // First call: file doesn't exist → fresh identifier created
        // + persisted. Second call: file exists → restored from disk.
        // The two identifiers must produce byte-identical
        // dataRepresentation (proof that persistence + restore is
        // lossless across the VZ FFI boundary).
        let tmp = std::env::temp_dir().join(format!(
            "kasou-machine-id-test-{}.bin",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&tmp);

        let platform = unsafe { VZGenericPlatformConfiguration::new() };
        let first = load_or_create_machine_identifier(&tmp, &platform).unwrap();
        assert!(tmp.exists(), "first call should persist the identifier");
        let first_bytes = unsafe { first.dataRepresentation() }.to_vec();

        // Second call — should restore from the persisted file.
        let platform2 = unsafe { VZGenericPlatformConfiguration::new() };
        let second = load_or_create_machine_identifier(&tmp, &platform2).unwrap();
        let second_bytes = unsafe { second.dataRepresentation() }.to_vec();

        assert_eq!(
            first_bytes, second_bytes,
            "restored identifier must match the persisted one byte-for-byte"
        );

        // Cleanup.
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn machine_identifier_rejects_corrupt_file() {
        let tmp = std::env::temp_dir().join(format!(
            "kasou-corrupt-id-{}.bin",
            std::process::id()
        ));
        std::fs::write(&tmp, b"not a real machine identifier").unwrap();
        let platform = unsafe { VZGenericPlatformConfiguration::new() };
        let res = load_or_create_machine_identifier(&tmp, &platform);
        assert!(
            res.is_err(),
            "corrupt identifier file should fail loudly, not silently regenerate"
        );
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn machine_identifier_creates_parent_dir() {
        let tmp = std::env::temp_dir().join(format!(
            "kasou-nested-{}-vmdir/machine-id.bin",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(tmp.parent().unwrap());
        let platform = unsafe { VZGenericPlatformConfiguration::new() };
        let _id = load_or_create_machine_identifier(&tmp, &platform).unwrap();
        assert!(tmp.exists());
        let _ = std::fs::remove_dir_all(tmp.parent().unwrap());
    }

    #[test]
    fn validate_rejects_no_disks() {
        let config = VmConfig {
            id: crate::types::VmId::default(),
            cpus: 1,
            memory_mib: 1024,
            boot: BootConfig::Linux {
                kernel: PathBuf::from("/nonexistent/kernel"),
                initrd: PathBuf::from("/nonexistent/initrd"),
                cmdline: String::new(),
            },
            disks: vec![],
            network: NetworkConfig { mac_address: None },
            serial: None,
            shared_dirs: vec![],
            machine_identifier_path: None,
        };
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("disk"));
    }
}
