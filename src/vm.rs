use std::sync::Arc;

use block2::RcBlock;
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
/// SAFETY: All `VZVirtualMachine` interactions are dispatched to its serial
/// queue via `exec_sync`/`exec_async`. The wrapper itself is only moved
/// between tokio tasks — never accessed concurrently without queue serialization.
struct SendVm(Retained<VZVirtualMachine>);
unsafe impl Send for SendVm {}

/// Handle to a virtual machine.
///
/// Owns the `VZVirtualMachine` instance and its delegate. All VM operations
/// use the VM's associated dispatch queue for thread safety. Completion
/// handlers bridge to tokio via oneshot channels.
pub struct VmHandle {
    vm: SendVm,
    _delegate: Retained<VmDelegate>,
    state_rx: watch::Receiver<VmState>,
}

// SAFETY: VmHandle is Send because:
// - SendVm wraps the VM with the guarantee that all access is serialized
// - The delegate is a Retained objc object only accessed from the queue
// - watch::Receiver is Send
unsafe impl Send for VmHandle {}

impl VmHandle {
    /// Create a new VM from configuration.
    ///
    /// Validates the config, builds the `VZVirtualMachineConfiguration`,
    /// creates the VM on the main dispatch queue, and sets up the delegate.
    pub fn create(vm_config: VmConfig) -> Result<Self, KasouError> {
        vm_config.validate()?;

        let vz_config = config::build_vz_config(&vm_config)?;

        let (state_tx, state_rx) = watch::channel(VmState::Stopped);
        let state_tx = Arc::new(state_tx);
        let delegate = VmDelegate::new(Arc::clone(&state_tx));

        // SAFETY: initWithConfiguration creates a VM using the main queue.
        // All subsequent operations must happen on the main queue.
        let vm = unsafe {
            VZVirtualMachine::initWithConfiguration(VZVirtualMachine::alloc(), &vz_config)
        };

        // SAFETY: setDelegate assigns the delegate for state change callbacks.
        unsafe {
            vm.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        };

        Ok(Self {
            vm: SendVm(vm),
            _delegate: delegate,
            state_rx,
        })
    }

    /// Start the virtual machine.
    ///
    /// The VM must be in Stopped or Error state.
    /// The completion handler signals success/failure via a oneshot channel.
    pub async fn start(&self) -> Result<(), KasouError> {
        let can_start = unsafe { self.vm.0.canStart() };
        if !can_start {
            let state = unsafe { self.vm.0.state() };
            return Err(KasouError::InvalidState {
                current: VmState::from_vz(state).to_string(),
                expected: "stopped or error".into(),
            });
        }

        let (tx, rx) = tokio::sync::oneshot::channel();
        let tx = std::sync::Mutex::new(Some(tx));

        let block = RcBlock::new(move |error: *mut NSError| {
            let result = if error.is_null() {
                Ok(())
            } else {
                // SAFETY: error is a valid NSError pointer when non-null.
                let desc = unsafe { (*error).localizedDescription() }.to_string();
                Err(KasouError::OperationFailed(format!("start failed: {desc}")))
            };
            if let Some(tx) = tx.lock().unwrap().take() {
                let _ = tx.send(result);
            }
        });

        // SAFETY: startWithCompletionHandler begins VM execution.
        unsafe { self.vm.0.startWithCompletionHandler(&block) };

        rx.await.map_err(|_| KasouError::QueueCancelled)?
    }

    /// Stop the virtual machine (hard stop).
    ///
    /// Destructive — the guest does not get a chance to shut down cleanly.
    /// Use `request_stop()` for graceful shutdown.
    pub async fn stop(&self) -> Result<(), KasouError> {
        let can_stop = unsafe { self.vm.0.canStop() };
        if !can_stop {
            let state = unsafe { self.vm.0.state() };
            return Err(KasouError::InvalidState {
                current: VmState::from_vz(state).to_string(),
                expected: "running or paused".into(),
            });
        }

        let (tx, rx) = tokio::sync::oneshot::channel();
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

        // SAFETY: stopWithCompletionHandler terminates the VM.
        unsafe { self.vm.0.stopWithCompletionHandler(&block) };

        rx.await.map_err(|_| KasouError::QueueCancelled)?
    }

    /// Request the guest to stop (graceful shutdown via ACPI power button).
    pub fn request_stop(&self) -> Result<(), KasouError> {
        // SAFETY: requestStopWithError sends a power button event to the guest.
        unsafe { self.vm.0.requestStopWithError() }.map_err(|e| {
            let desc = e.localizedDescription().to_string();
            KasouError::OperationFailed(format!("request stop failed: {desc}"))
        })
    }

    /// Pause the virtual machine.
    pub async fn pause(&self) -> Result<(), KasouError> {
        let can_pause = unsafe { self.vm.0.canPause() };
        if !can_pause {
            let state = unsafe { self.vm.0.state() };
            return Err(KasouError::InvalidState {
                current: VmState::from_vz(state).to_string(),
                expected: "running".into(),
            });
        }

        let (tx, rx) = tokio::sync::oneshot::channel();
        let tx = std::sync::Mutex::new(Some(tx));

        let block = RcBlock::new(move |error: *mut NSError| {
            let result = if error.is_null() {
                Ok(())
            } else {
                let desc = unsafe { (*error).localizedDescription() }.to_string();
                Err(KasouError::OperationFailed(format!("pause failed: {desc}")))
            };
            if let Some(tx) = tx.lock().unwrap().take() {
                let _ = tx.send(result);
            }
        });

        // SAFETY: pauseWithCompletionHandler suspends the VM.
        unsafe { self.vm.0.pauseWithCompletionHandler(&block) };

        rx.await.map_err(|_| KasouError::QueueCancelled)?
    }

    /// Resume a paused virtual machine.
    pub async fn resume(&self) -> Result<(), KasouError> {
        let can_resume = unsafe { self.vm.0.canResume() };
        if !can_resume {
            let state = unsafe { self.vm.0.state() };
            return Err(KasouError::InvalidState {
                current: VmState::from_vz(state).to_string(),
                expected: "paused".into(),
            });
        }

        let (tx, rx) = tokio::sync::oneshot::channel();
        let tx = std::sync::Mutex::new(Some(tx));

        let block = RcBlock::new(move |error: *mut NSError| {
            let result = if error.is_null() {
                Ok(())
            } else {
                let desc = unsafe { (*error).localizedDescription() }.to_string();
                Err(KasouError::OperationFailed(format!("resume failed: {desc}")))
            };
            if let Some(tx) = tx.lock().unwrap().take() {
                let _ = tx.send(result);
            }
        });

        // SAFETY: resumeWithCompletionHandler resumes the VM from paused state.
        unsafe { self.vm.0.resumeWithCompletionHandler(&block) };

        rx.await.map_err(|_| KasouError::QueueCancelled)?
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
