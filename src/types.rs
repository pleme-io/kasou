//! Core platform types for kasou.
//!
//! These types form the public data model of the kasou VM platform.
//! All are pure data — no I/O, no framework calls, fully testable.

use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Unique identifier for a virtual machine.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VmId(pub String);

impl fmt::Display for VmId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&str> for VmId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl From<String> for VmId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// 6-byte MAC address with locally-administered generation.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MacAddress(pub [u8; 6]);

impl MacAddress {
    /// Generate a deterministic locally-administered MAC from a seed and unique ID.
    ///
    /// The seed should be stable across reboots (e.g., hostname, machine-id).
    /// Follows Lima convention: `52:55:55:xx:xx:xx` prefix.
    ///
    /// ```
    /// use kasou::MacAddress;
    /// let mac = MacAddress::deterministic("my-host", "my-vm");
    /// assert!(mac.to_string().starts_with("52:55:55:"));
    /// ```
    pub fn deterministic(seed: &str, unique_id: &str) -> Self {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        seed.hash(&mut hasher);
        unique_id.hash(&mut hasher);
        let hash = hasher.finish().to_le_bytes();

        Self([0x52, 0x55, 0x55, hash[0], hash[1], hash[2]])
    }

    /// Parse a MAC address from colon-separated string (e.g., "5a:94:ef:ab:cd:12").
    pub fn parse(s: &str) -> Result<Self, crate::KasouError> {
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() != 6 {
            return Err(crate::KasouError::InvalidMac(s.to_string()));
        }
        let mut bytes = [0u8; 6];
        for (i, part) in parts.iter().enumerate() {
            bytes[i] = u8::from_str_radix(part, 16)
                .map_err(|_| crate::KasouError::InvalidMac(s.to_string()))?;
        }
        Ok(Self(bytes))
    }
}

impl fmt::Display for MacAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            self.0[0], self.0[1], self.0[2], self.0[3], self.0[4], self.0[5]
        )
    }
}

/// Network attachment strategy.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum NetworkAttachment {
    /// NAT networking — VM gets a 192.168.64.x address via macOS DHCP.
    Nat,
    /// Bridged networking — VM gets an IP on the host's LAN.
    /// Requires `com.apple.vm.networking` entitlement.
    Bridged { interface: String },
}

/// vsock configuration for host<>guest communication without networking.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VsockConfig {
    /// Context ID (CID) for the guest. Must be >= 3.
    pub cid: u32,
    /// Host-side Unix socket path for connections.
    pub socket_path: Option<PathBuf>,
}

/// Memory balloon configuration for dynamic memory management.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BalloonConfig {
    /// Release memory back to host when guest has OOM pressure.
    pub deflate_on_oom: bool,
}

/// Runtime information about a VM.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VmInfo {
    pub id: VmId,
    pub state: crate::vm::VmState,
    pub pid: Option<u32>,
    pub uptime_secs: Option<u64>,
    pub mac_address: Option<MacAddress>,
    pub ip_address: Option<String>,
}

/// VM metrics snapshot.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct VmMetrics {
    pub cpu_usage_percent: f64,
    pub memory_used_mib: u64,
    pub memory_total_mib: u64,
    pub uptime_secs: u64,
    pub disk: Vec<DiskMetrics>,
    pub network: Vec<NetworkMetrics>,
}

/// Per-disk I/O metrics.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DiskMetrics {
    pub id: String,
    pub read_bytes: u64,
    pub write_bytes: u64,
}

/// Per-interface network metrics.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct NetworkMetrics {
    pub id: String,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_address_display() {
        let mac = MacAddress([0x5a, 0x94, 0xef, 0xab, 0xcd, 0x12]);
        assert_eq!(mac.to_string(), "5a:94:ef:ab:cd:12");
    }

    #[test]
    fn mac_address_parse() {
        let mac = MacAddress::parse("5a:94:ef:ab:cd:12").unwrap();
        assert_eq!(mac.0, [0x5a, 0x94, 0xef, 0xab, 0xcd, 0x12]);
    }

    #[test]
    fn mac_address_parse_invalid() {
        assert!(MacAddress::parse("not-a-mac").is_err());
        assert!(MacAddress::parse("5a:94:ef:ab:cd").is_err());
        assert!(MacAddress::parse("5a:94:ef:ab:cd:12:ff").is_err());
        assert!(MacAddress::parse("gg:94:ef:ab:cd:12").is_err());
        // Verify it returns KasouError::InvalidMac
        match MacAddress::parse("bad") {
            Err(crate::KasouError::InvalidMac(s)) => assert_eq!(s, "bad"),
            other => panic!("expected InvalidMac, got {other:?}"),
        }
    }

    #[test]
    fn mac_address_deterministic_stable() {
        let a = MacAddress::deterministic("host", "cid-k3s");
        let b = MacAddress::deterministic("host", "cid-k3s");
        assert_eq!(a, b);
        assert_eq!(a.0[0], 0x52); // locally-administered prefix
        assert_eq!(a.0[1], 0x55);
        assert_eq!(a.0[2], 0x55);
    }

    #[test]
    fn mac_address_deterministic_differs() {
        let a = MacAddress::deterministic("host", "cid-k3s");
        let b = MacAddress::deterministic("host", "ryn-k3s");
        assert_ne!(a, b);
    }

    #[test]
    fn vm_id_from_str() {
        let id: VmId = "my-vm".into();
        assert_eq!(id.0, "my-vm");
        assert_eq!(id.to_string(), "my-vm");
    }
}
