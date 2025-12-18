use std::fs::{self, File};
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use tempfile::tempdir;
use vm_memory::{GuestAddress, GuestMemoryMmap};

use rust_hypervisor::virtio::fs::{VirtioFs, Acl, AclEntry, AclPermissions};
use rust_hypervisor::error::Result;

#[test]
fn test_basic_fs_operations() -> Result<()> {
    // Create a temporary directory for testing
    let temp_dir = tempdir()?;
    let shared_dir = temp_dir.path().join("shared");
    fs::create_dir(&shared_dir)?;
    
    // Create a test file
    let test_file = shared_dir.join("test.txt");
    let mut file = File::create(&test_file)?;
    writeln!(file, "Hello, Virtio-FS!")?;
    
    // Initialize memory and create filesystem
    let mem = Arc::new(GuestMemoryMmap::<()>::from_ranges(&[(
        GuestAddress(0x0),
        0x10000,
    )])?);
    let mmio_base = 0x1000;
    let irq = 5;
    
    let mut fs = VirtioFs::new(
        &shared_dir,
        mem,
        mmio_base.into(),
        irq,
        "test",
        false,
        Some((mmio_base, 4096)),
    )?;
    
    // Set up a simple ACL
    let acl = Acl {
        owner: 1000,
        group: 1000,
        entries: vec![
            AclEntry {
                uid: 1000,
                gid: 1000,
                permissions: AclPermissions::all(),
            },
        ],
    };
    
    fs.set_acl(Path::new("/"), acl)?;
    
    // Test MMIO read/write
    let test_data = [0x12, 0x34, 0x56, 0x78];
    let mut read_buf = [0u8; 4];
    
    // Write to MMIO
    fs.mmio_write(0x100, &test_data)?;
    
    // Read back from MMIO
    fs.mmio_read(0x100, &mut read_buf)?;
    
    assert_eq!(test_data, read_buf);
    
    // Clean up
    drop(file);
    fs::remove_dir_all(&shared_dir)?;
    
    Ok(())
}
