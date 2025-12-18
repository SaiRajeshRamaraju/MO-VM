use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use vm_memory::{GuestAddress, GuestMemoryMmap};

use rust_hypervisor::virtio::fs::{VirtioFs, Acl, AclEntry, AclPermissions};
use rust_hypervisor::error::Result;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Starting Virtio-FS test...");

    // Create a test directory
    let test_dir = "test_share";
    fs::create_dir_all(test_dir)?;

    // Create a test file
    let test_file = format!("{}/test_file.txt", test_dir);
    fs::write(&test_file, "Hello from host system!\n")?;

    // Initialize memory
    let mem = Arc::new(
        GuestMemoryMmap::<()>::from_ranges(&[(GuestAddress(0), 0x10000)])
            .expect("Failed to create guest memory"),
    );

    // Create Virtio-FS device
    let mut fs = VirtioFs::new(
        test_dir,
        mem,
        GuestAddress(0x1000),
        5, // irq
        "test",
        false,
        Some((0x1000, 0x1000)),
    )
    .expect("Failed to create Virtio-FS device");

    println!("Virtio-FS device created successfully");

    // Test MMIO operations
    let test_data = [0x12, 0x34, 0x56, 0x78];
    let mut read_buf = [0u8; 4];

    println!("Testing MMIO write...");
    fs.mmio_write(0x100, &test_data)
        .expect("Failed to write to MMIO");

    println!("Testing MMIO read...");
    fs.mmio_read(0x100, &mut read_buf).expect("Failed to read from MMIO");

    assert_eq!(test_data, read_buf);
    println!("MMIO test passed!");

    // Test ACL operations
    println!("Testing ACL operations...");
    let acl = Acl {
        owner: 1000,
        group: 1000,
        entries: vec![
            AclEntry {
                uid: 1000,
                gid: 1000,
                permissions: AclPermissions::all(),
            },
            AclEntry {
                uid: 1001,
                gid: 1001,
                permissions: AclPermissions::READ | AclPermissions::EXECUTE,
            },
        ],
    };

    fs.set_acl(Path::new("/"), acl).expect("Failed to set ACL");
    println!("ACL test passed!");

    // Run the shell script test
    println!("\nRunning shell script test...");
    let output = Command::new("bash")
        .arg("tests/fs_test.sh")
        .output()
        .expect("Failed to execute test script");

    println!("Test script output:");
    io::stdout().write_all(&output.stdout).unwrap();
    io::stderr().write_all(&output.stderr).unwrap();

    if output.status.success() {
        println!("\n✅ All tests passed!");
        Ok(())
    } else {
        eprintln!("\n❌ Tests failed");
        std::process::exit(1);
    }
}
