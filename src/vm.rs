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

/// Send-safe raw pointer to VZVirtualMachine for dispatch queue closures.
///
/// SAFETY: The pointer is valid for the lifetime of the owning VmHandle.
/// All access happens on the VM's serial dispatch queue.
struct SendVmPtr(*const VZVirtualMachine);
unsafe impl Send for SendVmPtr {}

/// Handle to a virtual machine.
///
/// Owns the `VZVirtualMachine` instance, its dedicated dispatch queue, and
/// the delegate. VM operations call the framework API directly — the
/// framework dispatches to the VM's queue internally. Completions are
/// bridged back via `std::sync::mpsc` channels.
pub struct VmHandle {
    vm: SendVm,
    _queue: DispatchRetained<DispatchQueue>,
    _delegate: Retained<VmDelegate>,
    state_rx: watch::Receiver<VmState>,
}

// SAFETY: VmHandle is Send because the framework serializes all VM access
// through its internal dispatch queue.
unsafe impl Send for VmHandle {}

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

        // Create a dedicated serial dispatch queue for this VM.
        // The framework dispatches all VM operations and delegate callbacks here.
        let queue = DispatchQueue::new("io.pleme.kasou.vm", None);

        tracing::info!("creating VZVirtualMachine with dedicated dispatch queue");
        // SAFETY: initWithConfiguration_queue creates a VM bound to our serial queue.
        // The configuration has been validated via validateWithError in build_vz_config.
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
            _queue: queue,
            _delegate: delegate,
            state_rx,
        })
    }

    /// Start the virtual machine.
    ///
    /// Calls `startWithCompletionHandler:` — the framework dispatches to the
    /// VM's queue internally. Blocks the calling thread until completion.
    pub fn start(&self) -> Result<(), KasouError> {
        let (tx, rx) = std::sync::mpsc::channel();
        // Cast pointer to usize (which is Send) to move into the closure.
        // SAFETY: the pointer is valid for the lifetime of VmHandle, and
        // we block on rx.recv() below, so VmHandle cannot be dropped.
        let vm_addr = &*self.vm.0 as *const VZVirtualMachine as usize;

        self._queue.exec_async(move || {
            let tx = std::sync::Mutex::new(Some(tx));
            let block = RcBlock::new(move |error: *mut NSError| {
                let result = if error.is_null() {
                    Ok(())
                } else {
                    let desc = unsafe { (*error).localizedDescription() }.to_string();
                    Err(KasouError::OperationFailed(format!("start failed: {desc}")))
                };
                if let Some(tx) = tx.lock().unwrap().take() {
                    let _ = tx.send(result);
                }
            });

            let vm_ptr = vm_addr as *const VZVirtualMachine;
            unsafe { (*vm_ptr).startWithCompletionHandler(&block) };
        });

        rx.recv().map_err(|_| KasouError::QueueCancelled)?
    }

    /// Stop the virtual machine (hard stop).
    ///
    /// Destructive — the guest does not get a chance to shut down cleanly.
    /// Use `request_stop()` for graceful shutdown.
    pub fn stop(&self) -> Result<(), KasouError> {
        let (tx, rx) = std::sync::mpsc::channel();
        let vm_addr = &*self.vm.0 as *const VZVirtualMachine as usize;

        self._queue.exec_async(move || {
            let tx = std::sync::Mutex::new(Some(tx));
            let block = RcBlock::new(move |error: *mut NSError| {
                let result = if error.is_null() {
                    Ok(())
                } else {
                    let desc = unsafe { (*error).localizedDescription() }.to_string();
                    Err(KasouError::OperationFailed(format!("stop failed: {desc}")))
                };
                if let Some(tx) = tx.lock().unwrap().take() {
                    let _ = tx.send(result);
                }
            });

            let vm_ptr = vm_addr as *const VZVirtualMachine;
            unsafe { (*vm_ptr).stopWithCompletionHandler(&block) };
        });

        rx.recv().map_err(|_| KasouError::QueueCancelled)?
    }

    /// Request the guest to stop (graceful shutdown via ACPI power button).
    pub fn request_stop(&self) -> Result<(), KasouError> {
        // SAFETY: requestStopWithError sends a power button event to the guest.
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
