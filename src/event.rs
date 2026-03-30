//! VM lifecycle event system.
//!
//! Events are broadcast to multiple subscribers via `tokio::sync::broadcast`.
//! Subscribers can lag without blocking the VM runtime — they receive
//! `RecvError::Lagged(n)` and can recover.

use std::path::PathBuf;
use std::time::Instant;

use tokio::sync::broadcast;

use crate::types::{MacAddress, VmId};
use crate::vm::VmState;

/// A VM lifecycle event.
#[derive(Clone, Debug)]
pub struct VmEvent {
    pub timestamp: Instant,
    pub vm_id: VmId,
    pub kind: VmEventKind,
}

/// The kind of VM event that occurred.
#[derive(Clone, Debug)]
pub enum VmEventKind {
    /// VM state changed.
    StateChanged { from: VmState, to: VmState },
    /// VM finished booting (kernel + init reached).
    BootComplete { duration_ms: u64 },
    /// Graceful shutdown was requested (ACPI power button).
    ShutdownRequested,
    /// VM crashed with an error.
    Crashed { reason: String },
    /// Network interface came up with assigned MAC.
    NetworkUp { mac: MacAddress },
    /// Network interface went down.
    NetworkDown { reason: String },
    /// Snapshot was created.
    SnapshotCreated { path: PathBuf },
    /// Snapshot was restored.
    SnapshotRestored { path: PathBuf },
    /// VM was force-stopped (Drop guard or hard stop).
    ForceStop,
}

/// Broadcast event bus for VM lifecycle events.
///
/// Multiple consumers (CLI, daemon, metrics, MCP server) can subscribe
/// independently. Slow consumers get `Lagged` errors rather than
/// blocking the VM runtime.
pub struct VmEventBus {
    tx: broadcast::Sender<VmEvent>,
}

impl VmEventBus {
    /// Create a new event bus with the given channel capacity.
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }

    /// Emit an event to all subscribers.
    pub fn emit(&self, event: VmEvent) {
        let _ = self.tx.send(event);
    }

    /// Emit a state change event.
    pub fn state_changed(&self, vm_id: VmId, from: VmState, to: VmState) {
        self.emit(VmEvent {
            timestamp: Instant::now(),
            vm_id,
            kind: VmEventKind::StateChanged { from, to },
        });
    }

    /// Subscribe to events. Returns a receiver that can be polled.
    pub fn subscribe(&self) -> broadcast::Receiver<VmEvent> {
        self.tx.subscribe()
    }
}

impl Default for VmEventBus {
    fn default() -> Self {
        Self::new(256)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::VmId;
    use crate::vm::VmState;

    #[test]
    fn event_bus_emits_to_subscriber() {
        let bus = VmEventBus::default();
        let mut rx = bus.subscribe();

        bus.state_changed(VmId::from("test-vm"), VmState::Stopped, VmState::Running);

        let event = rx.try_recv().unwrap();
        assert_eq!(event.vm_id, VmId::from("test-vm"));
        match event.kind {
            VmEventKind::StateChanged { from, to } => {
                assert_eq!(from, VmState::Stopped);
                assert_eq!(to, VmState::Running);
            }
            other => panic!("expected StateChanged, got {other:?}"),
        }
    }

    #[test]
    fn event_bus_no_subscribers_does_not_panic() {
        let bus = VmEventBus::default();
        // Emitting with no subscribers should not panic
        bus.state_changed(VmId::from("x"), VmState::Running, VmState::Stopped);
    }

    #[test]
    fn event_bus_multiple_subscribers() {
        let bus = VmEventBus::default();
        let mut rx1 = bus.subscribe();
        let mut rx2 = bus.subscribe();

        bus.state_changed(VmId::from("vm"), VmState::Running, VmState::Paused);

        assert!(rx1.try_recv().is_ok());
        assert!(rx2.try_recv().is_ok());
    }
}
