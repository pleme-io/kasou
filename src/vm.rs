use std::cell::Cell;
use std::sync::Arc;

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::AnyThread;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::NSError;
use objc2_virtualization::{VZVirtualMachine, VZVirtualMachineState};
use tokio::sync::watch;

use crate::config::{self, VmConfig};
use crate::delegate::VmDelegate;
use crate::KasouError;

/// VM lifecycle state as observed from Rust.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmState {
    Stopped,
    Starting,
    Running,
    Pausing,
    Paused,
    Resuming,
    Stopping,
    Error,
}

impl VmState {
    pub(crate) fn from_vz(state: VZVirtualMachineState) -> Self {
        match state {
            VZVirtualMachineState::Stopped => Self::Stopped,
            VZVirtualMachineState::Starting => Self::Starting,
            VZVirtualMachineState::Running => Self::Running,
            VZVirtualMachineState::Pausing => Self::Pausing,
            VZVirtualMachineState::Paused => Self::Paused,
            VZVirtualMachineState::Resuming => Self::Resuming,
            VZVirtualMachineState::Stopping => Self::Stopping,
            VZVirtualMachineState::Error => Self::Error,
            _ => Self::Error,
        }
    }

    /// Whether this state represents a running VM that needs cleanup.
    pub fn is_active(self) -> bool {
        matches!(
            self,
            Self::Starting | Self::Running | Self::Pausing | Self::Paused | Self::Resuming
        )
    }
}

impl std::fmt::Display for VmState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stopped => write!(f, "stopped"),
            Self::Starting => write!(f, "starting"),
            Self::Running => write!(f, "running"),
            Self::Pausing => write!(f, "pausing"),
            Self::Paused => write!(f, "paused"),
            Self::Resuming => write!(f, "resuming"),
            Self::Stopping => write!(f, "stopping"),
            Self::Error => write!(f, "error"),
        }
    }
}

/// Wrapper to make `Retained<VZVirtualMachine>` `Send`.
///
/// SAFETY: All `VZVirtualMachine` interactions happen via the framework's
/// dispatch queue. The wrapper is only moved between threads — the framework
/// handles thread safety internally.
struct SendVm(Retained<VZVirtualMachine>);
unsafe impl Send for SendVm {}

/// Handle to a virtual machine.
///
/// Owns the `VZVirtualMachine` instance, its dedicated dispatch queue, and
/// the delegate. VM operations are dispatched to the serial queue via
/// `exec_async` with raw pointer bridging. Completions return via `mpsc`.
///
/// Implements `Drop` to force-stop active VMs, preventing orphaned processes.
pub struct VmHandle {
    vm: SendVm,
    queue: DispatchRetained<DispatchQueue>,
    _delegate: Retained<VmDelegate>,
    state_rx: watch::Receiver<VmState>,
}

// SAFETY: VmHandle is Send because the framework serializes all VM access
// through its internal dispatch queue.
unsafe impl Send for VmHandle {}

impl Drop for VmHandle {
    fn drop(&mut self) {
        let state = *self.state_rx.borrow();
        if state.is_active() {
            tracing::warn!(state = %state, "VmHandle dropped while VM active, requesting stop");
            // request_stop is sync and lightweight — safe in Drop
            let _ = unsafe { self.vm.0.requestStopWithError() };
            // Give guest a brief moment to respond to ACPI power button
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
    }
}

impl VmHandle {
    /// Create a new VM from configuration.
    ///
    /// Validates the config, builds the `VZVirtualMachineConfiguration`,
    /// creates the VM bound to a dedicated serial dispatch queue, and
    /// sets up the delegate for state change notifications.
    pub fn create(vm_config: VmConfig) -> Result<Self, KasouError> {
        vm_config.validate()?;

        tracing::info!("building VZ configuration");
        let vz_config = config::build_vz_config(&vm_config)?;
        tracing::info!("VZ configuration built successfully");

        let (state_tx, state_rx) = watch::channel(VmState::Stopped);
        let state_tx = Arc::new(state_tx);
        let delegate = VmDelegate::new(Arc::clone(&state_tx));

        let queue = DispatchQueue::new("io.pleme.kasou.vm", None);

        tracing::info!("creating VZVirtualMachine with dedicated dispatch queue");
        // SAFETY: initWithConfiguration_queue creates a VM bound to our serial queue.
        let vm = unsafe {
            VZVirtualMachine::initWithConfiguration_queue(
                VZVirtualMachine::alloc(),
                &vz_config,
                &queue,
            )
        };

        tracing::info!("VZVirtualMachine created, setting delegate");
        // SAFETY: setDelegate assigns the delegate for state change callbacks.
        unsafe {
            vm.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        };

        Ok(Self {
            vm: SendVm(vm),
            queue,
            _delegate: delegate,
            state_rx,
        })
    }

    /// Start the virtual machine.
    ///
    /// Dispatches `startWithCompletionHandler:` on the VM's serial queue.
    /// Blocks the calling thread until the completion handler fires.
    pub fn start(&self) -> Result<(), KasouError> {
        let (tx, rx) = std::sync::mpsc::channel();
        // Cast to usize (Send) to move into the exec_async closure.
        // SAFETY: pointer valid for VmHandle lifetime; rx.recv() blocks
        // so the handle cannot be dropped before the operation completes.
        let vm_addr = &*self.vm.0 as *const VZVirtualMachine as usize;

        self.queue.exec_async(move || {
            // Cell instead of Mutex — serial queue guarantees single-thread access
            let tx = Cell::new(Some(tx));
            let block = RcBlock::new(move |error: *mut NSError| {
                let result = if error.is_null() {
                    Ok(())
                } else {
                    let desc = unsafe { (*error).localizedDescription() }.to_string();
                    Err(KasouError::OperationFailed(format!("start failed: {desc}")))
                };
                if let Some(tx) = tx.take() {
                    let _ = tx.send(result);
                }
            });

            let vm_ptr = vm_addr as *const VZVirtualMachine;
            // SAFETY: on the VM's serial queue as required by the framework.
            unsafe { (*vm_ptr).startWithCompletionHandler(&block) };
        });

        rx.recv().map_err(|_| KasouError::QueueCancelled)?
    }

    /// Stop the virtual machine (hard stop).
    ///
    /// Destructive — the guest does not get a chance to shut down cleanly.
    /// Use `request_stop()` for graceful shutdown with timeout escalation.
    pub fn stop(&self) -> Result<(), KasouError> {
        let (tx, rx) = std::sync::mpsc::channel();
        let vm_addr = &*self.vm.0 as *const VZVirtualMachine as usize;

        self.queue.exec_async(move || {
            let tx = Cell::new(Some(tx));
            let block = RcBlock::new(move |error: *mut NSError| {
                let result = if error.is_null() {
                    Ok(())
                } else {
                    let desc = unsafe { (*error).localizedDescription() }.to_string();
                    Err(KasouError::OperationFailed(format!("stop failed: {desc}")))
                };
                if let Some(tx) = tx.take() {
                    let _ = tx.send(result);
                }
            });

            let vm_ptr = vm_addr as *const VZVirtualMachine;
            unsafe { (*vm_ptr).stopWithCompletionHandler(&block) };
        });

        rx.recv().map_err(|_| KasouError::QueueCancelled)?
    }

    /// Request the guest to stop (graceful shutdown via ACPI power button).
    ///
    /// The guest may ignore this. Callers should implement a timeout and
    /// escalate to `stop()` (hard stop) if the guest doesn't respond.
    pub fn request_stop(&self) -> Result<(), KasouError> {
        unsafe { self.vm.0.requestStopWithError() }.map_err(|e| {
            let desc = e.localizedDescription().to_string();
            KasouError::OperationFailed(format!("request stop failed: {desc}"))
        })
    }

    /// Get the current VM state.
    pub fn state(&self) -> VmState {
        *self.state_rx.borrow()
    }

    /// Get a watch receiver for observing state changes.
    pub fn state_watch(&self) -> watch::Receiver<VmState> {
        self.state_rx.clone()
    }
}
