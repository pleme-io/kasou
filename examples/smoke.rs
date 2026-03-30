/// Minimal test: create a VZ config and validate it.
/// Run with: cargo run --example smoke -- /path/to/kernel /path/to/initrd /path/to/disk.raw
use std::path::PathBuf;

fn main() {
    eprintln!("=== kasou smoke test ===");

    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("Usage: smoke <kernel> <initrd> <disk.raw> [init] [--wait]");
        std::process::exit(1);
    }

    let kernel = PathBuf::from(&args[1]);
    let initrd = PathBuf::from(&args[2]);
    let disk = PathBuf::from(&args[3]);
    let init = args.get(4).cloned();
    let wait_mode = args.iter().any(|a| a == "--wait");

    eprintln!("kernel: {}", kernel.display());
    eprintln!("initrd: {}", initrd.display());
    eprintln!("disk:   {}", disk.display());

    let config = kasou::VmConfig {
        id: kasou::VmId::from("smoke-test"),
        cpus: 2,
        memory_mib: 2048,
        boot: kasou::BootConfig {
            kernel,
            initrd,
            cmdline: if let Some(ref init) = init {
                format!("console=hvc0 root=/dev/vda init={init}")
            } else {
                "console=hvc0".to_string()
            },
        },
        disks: vec![kasou::DiskConfig {
            path: disk,
            read_only: false,
        }],
        network: kasou::NetworkConfig {
            mac_address: Some("52:55:55:aa:bb:cc".to_string()),
        },
        serial: Some(kasou::SerialConfig {
            log_path: std::path::PathBuf::from("/tmp/kasou-minimal/console.log"),
        }),
        shared_dirs: vec![],
    };

    eprintln!("validating config...");
    config.validate().expect("config validation failed");

    eprintln!("creating VmHandle...");
    let handle = kasou::VmHandle::create(config).expect("VmHandle::create failed");

    eprintln!("starting VM...");
    handle.start().expect("start failed");

    eprintln!("VM started! State: {}", handle.state());

    if wait_mode {
        eprintln!("=== WAIT MODE: VM running, press Ctrl+C to stop ===");
        eprintln!("Check DHCP: cat /var/db/dhcpd_leases | grep 52:55:55");
        eprintln!("SSH: ssh -o StrictHostKeyChecking=no root@<IP>  (password: nixos)");
        loop {
            std::thread::sleep(std::time::Duration::from_secs(60));
            eprintln!("  state: {}", handle.state());
        }
    } else {
        eprintln!("Sleeping 5s...");
        std::thread::sleep(std::time::Duration::from_secs(5));
        eprintln!("Stopping VM...");
        handle.stop().expect("stop failed");
        eprintln!("=== smoke test passed ===");
    }
}
