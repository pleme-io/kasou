use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_virtualization::{VZFileSerialPortAttachment, VZVirtioConsoleDeviceSerialPortConfiguration};

use crate::util::{ns_error_description, path_to_nsurl};
use crate::KasouError;

/// Configuration for a serial console that logs to a file.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SerialConfig {
    /// Path to the file where console output will be written.
    pub log_path: std::path::PathBuf,
}

pub(crate) fn create_serial_port(
    config: &SerialConfig,
) -> Result<Retained<VZVirtioConsoleDeviceSerialPortConfiguration>, KasouError> {
    let url = path_to_nsurl(&config.log_path)?;

    let attachment = unsafe {
        VZFileSerialPortAttachment::initWithURL_append_error(
            VZFileSerialPortAttachment::alloc(),
            &url,
            true,
        )
    }
    .map_err(|e| {
        KasouError::Framework(format!(
            "failed to create serial port attachment for {}: {}",
            config.log_path.display(),
            ns_error_description(&e)
        ))
    })?;

    let serial = unsafe { VZVirtioConsoleDeviceSerialPortConfiguration::new() };
    unsafe { serial.setAttachment(Some(&attachment)) };

    Ok(serial)
}
