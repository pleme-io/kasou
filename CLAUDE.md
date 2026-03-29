# Kasou (仮想) — Apple Virtualization.framework Library

Safe Rust bindings for macOS VM management via `objc2-virtualization`.
Consumed by kikai as a Cargo dependency to replace vfkit.

## Why This Exists

vfkit (Go CLI) ignores the `mac=` parameter for NAT networking.
kasou calls `VZMACAddress::initWithString` directly, fixing MAC address
control at the hypervisor level for deterministic DHCP leases.

## Architecture

- **Library crate** — no CLI, no workspace
- **objc2-virtualization 0.3.2** — auto-generated from Xcode 16.4 SDK
- **dispatch2** / **block2** — Objective-C runtime bridge
- All VM operations use the main dispatch queue
- Completion handlers → `tokio::sync::oneshot`
- Delegate state changes → `tokio::sync::watch`

## Modules

| Module | Purpose |
|--------|---------|
| `config.rs` | `VmConfig` builder → `VZVirtualMachineConfiguration` |
| `vm.rs` | `VmHandle`: create, start, stop, pause, resume, state |
| `network.rs` | NAT + MAC address (THE FIX) |
| `boot.rs` | `VZLinuxBootLoader` (kernel, initrd, cmdline) |
| `disk.rs` | `VZVirtioBlockDeviceConfiguration` (raw images) |
| `serial.rs` | `VZFileSerialPortAttachment` (console log) |
| `shared_dir.rs` | `VZVirtioFileSystemDeviceConfiguration` (virtiofs) |
| `delegate.rs` | `VZVirtualMachineDelegate` via `define_class!` |
| `queue.rs` | Dispatch queue bridge (reserved for future use) |
| `error.rs` | `KasouError` (thiserror) |

## Key Types

```rust
pub struct VmConfig { cpus, memory_mib, boot, disks, network, serial, shared_dirs }
pub struct NetworkConfig { mac_address: Option<String> }  // THE FIX
pub struct VmHandle { /* owns VZVirtualMachine */ }
pub enum VmState { Stopped, Starting, Running, Pausing, Paused, Resuming, Stopping, Error }
```

## Usage (from kikai)

```rust
let config = kasou::VmConfig {
    cpus: 4,
    memory_mib: 8192,
    boot: kasou::BootConfig { kernel, initrd, cmdline },
    disks: vec![kasou::DiskConfig { path: root_disk, read_only: false }],
    network: kasou::NetworkConfig { mac_address: Some("5a:94:ef:ab:cd:12".into()) },
    serial: Some(kasou::SerialConfig { log_path }),
    shared_dirs: vec![],
};
let handle = kasou::VmHandle::create(config)?;
handle.start().await?;
// ... VM is running with correct MAC ...
handle.stop().await?;
```

## Build

```bash
cargo test          # 6 tests
cargo check         # zero warnings
nix build           # via buildRustPackage
```

## Conventions

- Edition 2024, rust-version 1.89.0, MIT
- clippy pedantic, release LTO
- `unsafe` blocks wrapped in safe functions with documented safety invariants
- macOS-only (Apple Virtualization.framework)
