use std::path::Path;

use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_foundation::{NSString, NSURL};
use objc2_virtualization::{VZFileSerialPortAttachment, VZVirtioConsoleDeviceSerialPortConfiguration};

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

    // SAFETY: Creates a file serial port attachment that writes guest console
    // output to the specified file. Append mode preserves previous logs.
    let attachment = unsafe {
        VZFileSerialPortAttachment::initWithURL_append_error(
            VZFileSerialPortAttachment::alloc(),
            &url,
            true, // append mode
        )
    }
    .map_err(|e| {
        KasouError::Framework(format!(
            "failed to create serial port attachment for {}: {}",
            config.log_path.display(),
            ns_error_description(&e)
        ))
    })?;

    // SAFETY: Creates a virtio console serial port from a valid attachment.
    let serial = unsafe { VZVirtioConsoleDeviceSerialPortConfiguration::new() };

    // SAFETY: setAttachment is valid on any VZSerialPortConfiguration subclass.
    unsafe { serial.setAttachment(Some(&attachment)) };

    Ok(serial)
}

fn path_to_nsurl(path: &Path) -> Result<Retained<NSURL>, KasouError> {
    let path_str = path.to_str().ok_or_else(|| {
        KasouError::InvalidConfig(format!("path is not valid UTF-8: {}", path.display()))
    })?;
    let ns_path = NSString::from_str(path_str);
    // SAFETY: initFileURLWithPath creates a file URL from a filesystem path.
    let url = NSURL::initFileURLWithPath(NSURL::alloc(), &ns_path);
    Ok(url)
}

fn ns_error_description(error: &objc2_foundation::NSError) -> String {
    error.localizedDescription().to_string()
}
