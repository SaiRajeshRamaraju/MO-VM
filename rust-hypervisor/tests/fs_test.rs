use std::fs::{self, File};
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use tempfile::tempdir;

use rust_hypervisor::virtio::fs::{
    Acl, AclEntry, AclPermissions, FsState, VirtioFs, Error as FsError
};
use vm_memory::{GuestAddress, GuestMemoryMmap};

#[test]
fn test_acl_permissions() {
    // Create a test ACL
    let acl = Acl {
        owner: 1000,
        group: 1000,
        entries: vec![
            AclEntry {
                uid: 1001,
                gid: 1001,
                permissions: AclPermissions::READ | AclPermissions::WRITE,
            },
            AclEntry {
                uid: 1002,
                gid: 1002,
                permissions: AclPermissions::READ,
            },
        ],
    };

    // Test owner permissions
    assert!(acl.check_permission(1000, 1000, AclPermissions::READ));
    assert!(acl.check_permission(1000, 1000, AclPermissions::WRITE));
    assert!(acl.check_permission(1000, 1000, AclPermissions::all()));

    // Test specific user permissions
    assert!(acl.check_permission(1001, 1001, AclPermissions::READ));
    assert!(acl.check_permission(1001, 1001, AclPermissions::WRITE));
    assert!(!acl.check_permission(1001, 1001, AclPermissions::EXECUTE));

    // Test read-only user
    assert!(acl.check_permission(1002, 1002, AclPermissions::READ));
    assert!(!acl.check_permission(1002, 1002, AclPermissions::WRITE));

    // Test unauthorized user
    assert!(!acl.check_permission(9999, 9999, AclPermissions::READ));
}

#[test]
fn test_mmio_operations() -> Result<(), FsError> {
    let temp_dir = tempdir()?;
    let mmio_region = Some((0x1000, 4096)); // 4KB MMIO region
    
    let fs_state = FsState::new(temp_dir.path(), mmio_region)?;
    
    // Test MMIO write and read
    let test_data = [0x12, 0x34, 0x56, 0x78];
    fs_state.mmio_write(0x100, &test_data)?;
    
    let mut read_buf = [0u8; 4];
    fs_state.mmio_read(0x100, &mut read_buf)?;
    
    assert_eq!(test_data, read_buf);
    
    // Test out of bounds access
    assert!(fs_state.mmio_write(4096, &[0]).is_err());
    assert!(fs_state.mmio_read(4096, &mut [0]).is_err());
    
    Ok(())
}

#[test]
fn test_virtio_fs_integration() -> Result<(), Box<dyn std::error::Error>> {
    // Create a temporary directory for testing
    let temp_dir = tempdir()?;
    let shared_dir = temp_dir.path().join("shared");
    fs::create_dir(&shared_dir)?;
    
    // Create a test file
    let test_file = shared_dir.join("test.txt");
    let mut file = File::create(&test_file)?;
    writeln!(file, "Hello, Virtio-FS!")?;
    
    // Initialize memory and create filesystem
    let mem = Arc::new(GuestMemoryMmap::<()>::from_ranges(&[(GuestAddress(0), 0x10000)])?);
    let mmio_base = 0x1000;
    let irq = 5;
    
    let mut fs = VirtioFs::new(
        &shared_dir,
        mem,
        GuestAddress(mmio_base),
        irq,
        "test",
        false,
        Some((mmio_base, 4096)),
    )?;
    
    // Test file operations through the virtio-fs interface
    // This is a simplified test - in a real test, we would send virtio-fs protocol messages
    
    // Cleanup
    drop(file);
    fs::remove_dir_all(&shared_dir)?;
    
    Ok(())
}

#[test]
fn test_acl_persistence() -> Result<(), FsError> {
    let temp_dir = tempdir()?;
    let fs_state = FsState::new(temp_dir.path(), None)?;
    
    let test_acl = Acl {
        owner: 1000,
        group: 1000,
        entries: vec![
            AclEntry {
                uid: 1001,
                gid: 1001,
                permissions: AclPermissions::READ,
            },
        ],
    };
    
    // Set and get ACL
    let test_path = Path::new("/test");
    fs_state.set_acl(test_path, test_acl.clone())?;
    
    let retrieved_acl = fs_state.get_acl(test_path).unwrap();
    assert_eq!(test_acl.owner, retrieved_acl.owner);
    assert_eq!(test_acl.entries.len(), retrieved_acl.entries.len());
    
    Ok(())
}
