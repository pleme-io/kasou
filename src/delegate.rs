use std::sync::Arc;

use objc2::rc::Retained;
use objc2::{DefinedClass, define_class, AllocAnyThread, msg_send};
use objc2_foundation::{NSError, NSObject, NSObjectProtocol};
use objc2_virtualization::{VZVirtualMachine, VZVirtualMachineDelegate};
use tokio::sync::watch;

use crate::vm::VmState;

/// Instance variables for the delegate, holding a channel sender
/// to bridge Objective-C callbacks to tokio.
pub(crate) struct DelegateIvars {
    state_tx: Arc<watch::Sender<VmState>>,
}

define_class!(
    // SAFETY:
    // - NSObject has no subclassing requirements.
    // - We do not implement Drop (DelegateIvars does not need special cleanup).
    // - The delegate methods only send values through a channel, which is safe.
    #[unsafe(super(NSObject))]
    #[name = "KasouVmDelegate"]
    #[ivars = DelegateIvars]
    pub(crate) struct VmDelegate;

    unsafe impl NSObjectProtocol for VmDelegate {}

    unsafe impl VZVirtualMachineDelegate for VmDelegate {
        #[unsafe(method(guestDidStopVirtualMachine:))]
        fn guest_did_stop(&self, _vm: &VZVirtualMachine) {
            tracing::info!("guest initiated VM stop");
            let _ = self.ivars().state_tx.send(VmState::Stopped);
        }

        #[unsafe(method(virtualMachine:didStopWithError:))]
        fn did_stop_with_error(&self, _vm: &VZVirtualMachine, error: &NSError) {
            let desc = error.localizedDescription().to_string();
            tracing::error!(error = %desc, "VM stopped with error");
            let _ = self.ivars().state_tx.send(VmState::Error);
        }
    }
);

impl VmDelegate {
    /// Create a new VM delegate that sends state changes through the given watch channel.
    pub(crate) fn new(state_tx: Arc<watch::Sender<VmState>>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(DelegateIvars { state_tx });
        // SAFETY: init is always valid for NSObject subclasses.
        unsafe { msg_send![super(this), init] }
    }
}
