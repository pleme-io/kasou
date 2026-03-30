//! Kasou (仮想) — safe Apple Virtualization.framework bindings for macOS VM management.
//!
//! Kasou provides a Rust API for creating and managing virtual machines on macOS
//! using Apple's Virtualization.framework via `objc2-virtualization`. It replaces
//! vfkit for use cases where direct MAC address control at the hypervisor level
//! is required.
//!
//! # Primary use case
//!
//! kikai (K3s cluster orchestrator) consumes kasou as a library dependency,
//! replacing `VfkitRuntime` with a native `KasouRuntime` that sets
//! `VZMACAddress` properly for deterministic DHCP leases.
//!
//! # Architecture
//!
//! - All VM operations are dispatched to a serial `DispatchQueue`
//! - Completion handlers bridge to tokio via oneshot channels
//! - State changes from the `VZVirtualMachineDelegate` are sent via `watch` channels

mod boot;
mod config;
mod delegate;
mod disk;
mod error;
mod network;
mod queue;
mod serial;
mod shared_dir;
pub mod vm;

pub use boot::BootConfig;
pub use config::VmConfig;
pub use disk::DiskConfig;
pub use error::KasouError;
pub use network::{deterministic_mac, NetworkConfig};
pub use serial::SerialConfig;
pub use shared_dir::SharedDirConfig;
pub use vm::{VmHandle, VmState};
