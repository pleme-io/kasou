//! Kasou (仮想) — Apple Virtualization.framework platform library.
//!
//! Kasou provides a Rust API for creating and managing virtual machines on macOS
//! using Apple's Virtualization.framework via `objc2-virtualization`. It serves
//! as the VM backend for tatara (workload orchestrator) and kikai (K3s lifecycle).
//!
//! # Platform API
//!
//! Kasou follows the pleme-io platform style:
//! - **Pure data types** (`VmConfig`, `MacAddress`, `VmId`) — fully testable, no I/O
//! - **Runtime types** (`VmHandle`, `VmState`) — own OS resources
//! - **Event system** (`VmEvent`, `VmEventBus`) — broadcast lifecycle events
//! - **Builder pattern** (`VmConfigBuilder`) — fluent, type-safe configuration
//! - **Trait extensibility** — consumers can observe state and events
//!
//! # Quick Start
//!
//! ```no_run
//! use kasou::VmConfigBuilder;
//! use std::path::PathBuf;
//!
//! let config = VmConfigBuilder::new("my-vm")
//!     .cpus(4)
//!     .memory_mib(8192)
//!     .boot(PathBuf::from("/path/to/kernel"))
//!     .initrd(PathBuf::from("/path/to/initrd"))
//!     .cmdline("console=hvc0 root=/dev/vda")
//!     .disk(PathBuf::from("/path/to/root.img"))
//!     .deterministic_mac("my-host")
//!     .build()
//!     .unwrap();
//!
//! let handle = kasou::VmHandle::create(config).unwrap();
//! handle.start().unwrap();
//! ```

mod boot;
pub mod builder;
mod config;
mod delegate;
mod disk;
mod error;
pub mod event;
mod network;
mod serial;
mod shared_dir;
mod util;
pub mod types;
pub mod vm;

// Core config types (pure data)
pub use boot::BootConfig;
pub use builder::VmConfigBuilder;
pub use config::VmConfig;
pub use disk::DiskConfig;
pub use network::NetworkConfig;
pub use serial::SerialConfig;
pub use shared_dir::SharedDirConfig;

// Platform types
pub use types::{
    BalloonConfig, DiskMetrics, MacAddress, NetworkAttachment,
    NetworkMetrics, VmId, VmInfo, VmMetrics, VsockConfig,
};

// Event system
pub use event::{VmEvent, VmEventBus, VmEventKind};

// Runtime types
pub use error::KasouError;
pub use vm::{VmHandle, VmState};

// Utilities
pub use network::deterministic_mac;
