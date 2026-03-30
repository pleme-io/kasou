/// Minimal test: create a VZ config and validate it.
/// Run with: cargo run --example smoke -- /path/to/kernel /path/to/initrd /path/to/disk.raw
use std::path::PathBuf;

fn main() {
    eprintln!("=== kasou smoke test ===");

    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("Usage: smoke <kernel> <initrd> <disk.raw>");
        std::process::exit(1);
    }

    let kernel = PathBuf::from(&args[1]);
    let initrd = PathBuf::from(&args[2]);
    let disk = PathBuf::from(&args[3]);

    eprintln!("kernel: {}", kernel.display());
    eprintln!("initrd: {}", initrd.display());
    eprintln!("disk:   {}", disk.display());

    let config = kasou::VmConfig {
        cpus: 2,
        memory_mib: 2048,
        boot: kasou::BootConfig {
            kernel,
            initrd,
            cmdline: "console=hvc0".to_string(),
        },
        disks: vec![kasou::DiskConfig {
            path: disk,
            read_only: false,
        }],
        network: kasou::NetworkConfig {
            mac_address: Some("5a:94:ef:ab:cd:12".to_string()),
        },
        serial: None,
        shared_dirs: vec![],
    };

    eprintln!("validating config...");
    config.validate().expect("config validation failed");

    eprintln!("creating VmHandle...");
    let handle = kasou::VmHandle::create(config).expect("VmHandle::create failed");

    eprintln!("starting VM...");
    handle.start().expect("start failed");

    eprintln!("VM started! State: {}", handle.state());
    eprintln!("Sleeping 5s...");
    std::thread::sleep(std::time::Duration::from_secs(5));

    eprintln!("Stopping VM...");
    handle.stop().expect("stop failed");

    eprintln!("=== smoke test passed ===");
}
