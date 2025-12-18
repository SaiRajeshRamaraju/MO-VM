use std::fs;
use std::path::Path;
use tempfile::tempdir;
use vm_memory::{GuestAddress, GuestMemoryMmap};
use rust_hypervisor::virtio::fs::VirtioFs;

#[test]
fn test_virtio_fs_basic() -> std::result::Result<(), Box<dyn std::error::Error>> {
    // Create a temporary directory
    let temp_dir = tempdir()?;
    let shared_dir = temp_dir.path().join("shared");
    fs::create_dir(&shared_dir)?;

    // Create a test file
    let test_file = shared_dir.join("test.txt");
    fs::write(&test_file, "Hello, Virtio-FS!")?;

    // Initialize memory
    let mem = GuestMemoryMmap::<()>::from_ranges(&[(GuestAddress(0), 0x10000)])?;

    // Create Virtio-FS device
    let mut fs = VirtioFs::new(
        &shared_dir,
        mem,
        GuestAddress(0x1000),
        5, // irq
        "test",
        false,
        Some((0x1000, 0x1000)),
    )?;

    // Test MMIO operations
    let test_data = [0x12, 0x34, 0x56, 0x78];
    let mut read_buf = [0u8; 4];

    // Test MMIO write
    fs.mmio_write(0x100, &test_data)?;

    // Test MMIO read
    fs.mmio_read(0x100, &mut read_buf)?;

    assert_eq!(test_data, read_buf, "MMIO read/write test failed");

    // Test file operations through the filesystem
    let file_content = fs::read_to_string(test_file)?;
    assert_eq!(file_content, "Hello, Virtio-FS!");

    // Test directory listing
    let entries: Vec<_> = fs::read_dir(&shared_dir)?.collect::<Result<Vec<_>, _>>()?;
    assert!(!entries.is_empty(), "Should have at least one entry");

    Ok(())
}
