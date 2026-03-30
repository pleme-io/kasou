use std::cell::Cell;
use std::path::Path;
use std::sync::Arc;

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::AnyThread;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::{NSError, NSString, NSURL};
use objc2_virtualization::{VZVirtualMachine, VZVirtualMachineState};
use tokio::sync::watch;

use crate::config::{self, VmConfig};
use crate::delegate::VmDelegate;
use crate::KasouError;

/// VM lifecycle state as observed from Rust.
///
/// Maps 1:1 to Apple's `VZVirtualMachineState` with validated transitions.
/// The Error state is **terminal** — the VM must be recreated, not recovered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum VmState {
    Stopped,
    Starting,
    Running,
    Pausing,
    Paused,
    Resuming,
    Stopping,
    Saving,
    Restoring,
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
            VZVirtualMachineState::Saving => Self::Saving,
            VZVirtualMachineState::Restoring => Self::Restoring,
            VZVirtualMachineState::Error => Self::Error,
            _ => Self::Error,
        }
    }

    /// Whether this state represents a running VM that needs cleanup.
    pub fn is_active(self) -> bool {
        matches!(
            self,
            Self::Starting
                | Self::Running
                | Self::Pausing
                | Self::Paused
                | Self::Resuming
                | Self::Saving
                | Self::Restoring
        )
    }

    /// Whether a transition to the target state is valid.
    #[allow(clippy::match_like_matches_macro)]
    pub fn can_transition_to(self, target: Self) -> bool {
        matches!(
            (self, target),
            // Normal lifecycle
            (Self::Stopped, Self::Starting)
            | (Self::Starting, Self::Running)
            | (Self::Running, Self::Pausing)
            | (Self::Pausing, Self::Paused)
            | (Self::Paused, Self::Resuming)
            | (Self::Resuming, Self::Running)
            // Shutdown paths
            | (Self::Running, Self::Stopping)
            | (Self::Paused, Self::Stopping)
            | (Self::Stopping, Self::Stopped)
            // Guest-initiated stop
            | (Self::Running, Self::Stopped)
            // Save/restore (macOS 14+)
            | (Self::Paused, Self::Saving)
            | (Self::Saving, Self::Paused)
            | (Self::Stopped, Self::Restoring)
            | (Self::Restoring, Self::Paused)
            // Error from any active state
            | (Self::Starting, Self::Error)
            | (Self::Running, Self::Error)
            | (Self::Pausing, Self::Error)
            | (Self::Paused, Self::Error)
            | (Self::Resuming, Self::Error)
            | (Self::Stopping, Self::Error)
            | (Self::Saving, Self::Error)
            | (Self::Restoring, Self::Error)
        )
    }

    /// Whether this is a terminal state (VM cannot be restarted).
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Error)
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
            Self::Saving => write!(f, "saving"),
            Self::Restoring => write!(f, "restoring"),
            Self::Error => write!(f, "error"),
        }
    }
}

/// Wrapper to make `Retained<VZVirtualMachine>` `Send`.
struct SendVm(Retained<VZVirtualMachine>);
unsafe impl Send for SendVm {}

/// Handle to a virtual machine.
///
/// Full lifecycle: create → start → pause/resume → save/restore → stop.
/// All operations dispatch to a serial queue. Completions bridge via `mpsc`.
/// Implements `Drop` to force-stop active VMs.
pub struct VmHandle {
    vm: SendVm,
    queue: DispatchRetained<DispatchQueue>,
    _delegate: Retained<VmDelegate>,
    state_rx: watch::Receiver<VmState>,
}

unsafe impl Send for VmHandle {}

impl Drop for VmHandle {
    fn drop(&mut self) {
        let state = *self.state_rx.borrow();
        if state.is_active() {
            tracing::warn!(state = %state, "VmHandle dropped while VM active, forcing hard stop");
            // Hard stop via the dispatch queue — safe because exec_sync
            // blocks until the queue drains, ensuring the VM is stopped
            // before VmHandle fields are deallocated.
            let vm_addr = &*self.vm.0 as *const VZVirtualMachine as usize;
            self.queue.exec_sync(move || {
                let vm_ptr = vm_addr as *const VZVirtualMachine;
                // SAFETY: on the VM's serial queue, pointer valid (we're in Drop,
                // self still alive during this call).
                let block = RcBlock::new(|_error: *mut NSError| {});
                unsafe { (*vm_ptr).stopWithCompletionHandler(&block) };
            });
            // Brief wait for the hard stop to take effect
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
}

/// Helper: dispatch a VZ completion-handler operation on the queue.
/// Returns the result from the completion handler via mpsc.
fn dispatch_vz_op(
    queue: &DispatchQueue,
    vm_addr: usize,
    op_name: &'static str,
    call: fn(*const VZVirtualMachine, &block2::DynBlock<dyn Fn(*mut NSError)>),
) -> Result<(), KasouError> {
    let (tx, rx) = std::sync::mpsc::channel();

    queue.exec_async(move || {
        let tx = Cell::new(Some(tx));
        let block = RcBlock::new(move |error: *mut NSError| {
            let result = if error.is_null() {
                Ok(())
            } else {
                let desc = unsafe { (*error).localizedDescription() }.to_string();
                Err(KasouError::OperationFailed(format!("{op_name} failed: {desc}")))
            };
            if let Some(tx) = tx.take() {
                let _ = tx.send(result);
            }
        });

        let vm_ptr = vm_addr as *const VZVirtualMachine;
        call(vm_ptr, &block);
    });

    rx.recv().map_err(|_| KasouError::QueueCancelled)?
}

/// Helper: dispatch a VZ URL-based operation (save/restore) on the queue.
/// The URL is created on the dispatch queue thread, avoiding lifetime issues.
fn dispatch_vz_url_op(
    queue: &DispatchQueue,
    vm_addr: usize,
    path: &Path,
    op_name: &'static str,
    call: fn(*const VZVirtualMachine, *const NSURL, &block2::DynBlock<dyn Fn(*mut NSError)>),
) -> Result<(), KasouError> {
    let path_string = path.to_str().ok_or_else(|| {
        KasouError::InvalidConfig(format!("path not UTF-8: {}", path.display()))
    })?.to_string();

    let (tx, rx) = std::sync::mpsc::channel();

    queue.exec_async(move || {
        // Create NSURL on the dispatch queue thread — no cross-thread lifetime issues
        let ns_path = NSString::from_str(&path_string);
        let url = NSURL::initFileURLWithPath(NSURL::alloc(), &ns_path);

        let tx = Cell::new(Some(tx));
        let block = RcBlock::new(move |error: *mut NSError| {
            let result = if error.is_null() {
                Ok(())
            } else {
                let desc = unsafe { (*error).localizedDescription() }.to_string();
                Err(KasouError::OperationFailed(format!("{op_name} failed: {desc}")))
            };
            if let Some(tx) = tx.take() {
                let _ = tx.send(result);
            }
        });

        let vm_ptr = vm_addr as *const VZVirtualMachine;
        call(vm_ptr, &*url as *const NSURL, &block);
    });

    rx.recv().map_err(|_| KasouError::QueueCancelled)?
}

impl VmHandle {
    /// Create a new VM from configuration.
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
        let vm = unsafe {
            VZVirtualMachine::initWithConfiguration_queue(
                VZVirtualMachine::alloc(),
                &vz_config,
                &queue,
            )
        };

        tracing::info!("VZVirtualMachine created, setting delegate");
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

    fn vm_addr(&self) -> usize {
        &*self.vm.0 as *const VZVirtualMachine as usize
    }

    /// Start the virtual machine.
    pub fn start(&self) -> Result<(), KasouError> {
        dispatch_vz_op(&self.queue, self.vm_addr(), "start", |vm, block| {
            unsafe { (*vm).startWithCompletionHandler(block) };
        })
    }

    /// Hard stop the virtual machine (destructive).
    pub fn stop(&self) -> Result<(), KasouError> {
        dispatch_vz_op(&self.queue, self.vm_addr(), "stop", |vm, block| {
            unsafe { (*vm).stopWithCompletionHandler(block) };
        })
    }

    /// Pause the virtual machine (VZ native, zero CPU usage).
    ///
    /// The VM must be in Running state. After pausing, the VM can be
    /// resumed, saved to disk, or stopped.
    pub fn pause(&self) -> Result<(), KasouError> {
        dispatch_vz_op(&self.queue, self.vm_addr(), "pause", |vm, block| {
            unsafe { (*vm).pauseWithCompletionHandler(block) };
        })
    }

    /// Resume a paused virtual machine.
    pub fn resume(&self) -> Result<(), KasouError> {
        dispatch_vz_op(&self.queue, self.vm_addr(), "resume", |vm, block| {
            unsafe { (*vm).resumeWithCompletionHandler(block) };
        })
    }

    /// Save VM state to a file (macOS 14+, requires VM to be paused).
    ///
    /// The VM remains in Paused state after saving. The state file can be
    /// used later to restore the VM to this exact point.
    pub fn save_state(&self, path: &Path) -> Result<(), KasouError> {
        dispatch_vz_url_op(&self.queue, self.vm_addr(), path, "save", |vm, url, block| {
            unsafe { (*vm).saveMachineStateToURL_completionHandler(&*url, block) };
        })
    }

    /// Restore VM state from a file (macOS 14+).
    ///
    /// The VM must be in Stopped state. After restoring, the VM will be
    /// in Paused state (ready to resume).
    pub fn restore_state(&self, path: &Path) -> Result<(), KasouError> {
        dispatch_vz_url_op(&self.queue, self.vm_addr(), path, "restore", |vm, url, block| {
            unsafe { (*vm).restoreMachineStateFromURL_completionHandler(&*url, block) };
        })
    }

    /// Request the guest to stop (graceful ACPI power button).
    ///
    /// Dispatched through the serial queue to maintain the Send safety invariant.
    /// The guest may ignore this — callers should implement timeout escalation.
    pub fn request_stop(&self) -> Result<(), KasouError> {
        let (tx, rx) = std::sync::mpsc::channel();
        let vm_addr = self.vm_addr();

        self.queue.exec_async(move || {
            let vm_ptr = vm_addr as *const VZVirtualMachine;
            let result = unsafe { (*vm_ptr).requestStopWithError() }.map_err(|e| {
                let desc = e.localizedDescription().to_string();
                KasouError::OperationFailed(format!("request stop failed: {desc}"))
            });
            let _ = tx.send(result);
        });

        rx.recv().map_err(|_| KasouError::QueueCancelled)?
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
