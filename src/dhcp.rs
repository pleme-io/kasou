//! DHCP lease lookup for macOS NAT networking.
//!
//! Apple's Virtualization.framework NAT networking uses the macOS vmnet
//! DHCP server, which records leases in `/var/db/dhcpd_leases`. This
//! module parses that file to discover VM IP addresses by MAC.

use std::path::Path;

const LEASE_FILE: &str = "/var/db/dhcpd_leases";

/// A parsed DHCP lease entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DhcpLease {
    pub ip_address: String,
    pub hw_address: String,
    pub name: Option<String>,
}

/// Look up a VM's IP address by its MAC address.
///
/// Reads `/var/db/dhcpd_leases` and finds the most recent lease
/// matching the given MAC (case-insensitive, ignores `1,` prefix).
pub fn lookup_ip_by_mac(mac: &str) -> Option<String> {
    let content = std::fs::read_to_string(LEASE_FILE).ok()?;
    let leases = parse_leases(&content);
    find_lease_by_mac(&leases, mac).map(|l| l.ip_address.clone())
}

/// Look up from a specific file (for testing and non-standard paths).
pub fn lookup_ip_by_mac_from(path: &Path, mac: &str) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    let leases = parse_leases(&content);
    find_lease_by_mac(&leases, mac).map(|l| l.ip_address.clone())
}

/// Parse the macOS DHCP lease file format.
///
/// The file contains blocks delimited by `{` and `}` with key=value pairs:
/// ```text
/// {
///   name=minimal
///   ip_address=192.168.64.9
///   hw_address=1,52:55:55:aa:bb:cc
///   identifier=1,52:55:55:aa:bb:cc
///   lease=0x69caa0ef
/// }
/// ```
pub fn parse_leases(content: &str) -> Vec<DhcpLease> {
    let mut leases = Vec::new();
    let mut current_ip = None;
    let mut current_hw = None;
    let mut current_name = None;

    for line in content.lines() {
        let line = line.trim();

        if line == "}" {
            if let (Some(ip), Some(hw)) = (current_ip.take(), current_hw.take()) {
                leases.push(DhcpLease {
                    ip_address: ip,
                    hw_address: hw,
                    name: current_name.take(),
                });
            }
            current_name = None;
        } else if let Some(val) = line.strip_prefix("ip_address=") {
            current_ip = Some(val.to_string());
        } else if let Some(val) = line.strip_prefix("hw_address=") {
            // Strip the "1," prefix that macOS adds (type indicator)
            let mac = val.strip_prefix("1,").unwrap_or(val);
            current_hw = Some(mac.to_string());
        } else if let Some(val) = line.strip_prefix("name=") {
            current_name = Some(val.to_string());
        }
    }

    leases
}

/// Find a lease by MAC address (case-insensitive).
pub fn find_lease_by_mac<'a>(leases: &'a [DhcpLease], mac: &str) -> Option<&'a DhcpLease> {
    let mac_lower = mac.to_lowercase();
    // Return the last matching lease (most recent in the file)
    leases.iter().rev().find(|l| l.hw_address.to_lowercase() == mac_lower)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_LEASES: &str = r#"{
	name=cid-k3s
	ip_address=192.168.64.8
	hw_address=1,5a:94:ef:5c:f4:70
	identifier=1,5a:94:ef:5c:f4:70
	lease=0x69caa111
}
{
	name=minimal
	ip_address=192.168.64.9
	hw_address=1,52:55:55:aa:bb:cc
	identifier=1,52:55:55:aa:bb:cc
	lease=0x69caa0ef
}
{
	name=cid-k3s
	ip_address=192.168.64.7
	hw_address=1,96:5b:2f:6a:46:ce
	identifier=1,96:5b:2f:6a:46:ce
	lease=0x69ca9c00
}
"#;

    #[test]
    fn parse_sample_leases() {
        let leases = parse_leases(SAMPLE_LEASES);
        assert_eq!(leases.len(), 3);
        assert_eq!(leases[0].ip_address, "192.168.64.8");
        assert_eq!(leases[0].hw_address, "5a:94:ef:5c:f4:70");
        assert_eq!(leases[0].name.as_deref(), Some("cid-k3s"));
    }

    #[test]
    fn find_by_mac_exact() {
        let leases = parse_leases(SAMPLE_LEASES);
        let found = find_lease_by_mac(&leases, "52:55:55:aa:bb:cc");
        assert_eq!(found.unwrap().ip_address, "192.168.64.9");
    }

    #[test]
    fn find_by_mac_case_insensitive() {
        let leases = parse_leases(SAMPLE_LEASES);
        let found = find_lease_by_mac(&leases, "52:55:55:AA:BB:CC");
        assert_eq!(found.unwrap().ip_address, "192.168.64.9");
    }

    #[test]
    fn find_by_mac_missing() {
        let leases = parse_leases(SAMPLE_LEASES);
        assert!(find_lease_by_mac(&leases, "00:00:00:00:00:00").is_none());
    }

    #[test]
    fn parse_empty_file() {
        let leases = parse_leases("");
        assert!(leases.is_empty());
    }

    #[test]
    fn parse_malformed_entry() {
        let content = "{\n\tip_address=1.2.3.4\n}\n";
        let leases = parse_leases(content);
        // Missing hw_address → entry skipped
        assert!(leases.is_empty());
    }

    #[test]
    fn hw_address_without_prefix() {
        let content = "{\n\tip_address=10.0.0.1\n\thw_address=aa:bb:cc:dd:ee:ff\n}\n";
        let leases = parse_leases(content);
        assert_eq!(leases.len(), 1);
        assert_eq!(leases[0].hw_address, "aa:bb:cc:dd:ee:ff");
    }

    #[test]
    fn returns_last_lease_for_mac() {
        // Same MAC appears twice with different IPs — last one wins
        let content = concat!(
            "{\n\tip_address=10.0.0.1\n\thw_address=1,aa:bb:cc:dd:ee:ff\n}\n",
            "{\n\tip_address=10.0.0.2\n\thw_address=1,aa:bb:cc:dd:ee:ff\n}\n",
        );
        let leases = parse_leases(content);
        let found = find_lease_by_mac(&leases, "aa:bb:cc:dd:ee:ff");
        assert_eq!(found.unwrap().ip_address, "10.0.0.2");
    }

    #[test]
    fn parse_name_field() {
        let leases = parse_leases(SAMPLE_LEASES);
        assert_eq!(leases[1].name.as_deref(), Some("minimal"));
    }
}
