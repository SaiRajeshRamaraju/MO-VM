use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::tempdir;
use vm_memory::{GuestAddress, GuestMemoryMmap};
use vmm_sys_util::eventfd::EventFd;

use rust_hypervisor::{
    virtio::fs::{VirtioFs, Acl, AclEntry, AclPermissions, FsState},
    virtio::VirtioDevice,
    error::Result,
};

/// Helper to create a test FsState with a temp directory.
fn create_test_fs_state() -> (tempfile::TempDir, Arc<std::sync::RwLock<FsState>>) {
    let temp_dir = tempdir().expect("Failed to create temp directory");
    let shared_dir = temp_dir.path().join("shared");
    fs::create_dir(&shared_dir).expect("Failed to create shared directory");
    
    let state = Arc::new(std::sync::RwLock::new(
        FsState::new(&shared_dir, None).expect("Failed to create FsState")
    ));
    
    (temp_dir, state)
}

// ==================== Filesystem Tests ====================

#[test]
fn test_basic_file_operations() -> Result<()> {
    let temp_dir = tempdir().expect("temp dir");
    let shared_dir = temp_dir.path().join("shared");
    fs::create_dir(&shared_dir).unwrap();
    
    // Test file creation and reading
    let test_content = "Test file content";
    let file_path = shared_dir.join("test.txt");
    fs::write(&file_path, test_content).unwrap();
    
    let content = fs::read_to_string(&file_path)?;
    assert_eq!(content, test_content);
    
    // Test file writing
    let new_content = "Updated content";
    fs::write(&file_path, new_content)?;
    
    let updated_content = fs::read_to_string(&file_path)?;
    assert_eq!(updated_content, new_content);
    
    Ok(())
}

#[test]
fn test_directory_operations() -> Result<()> {
    let temp_dir = tempdir().expect("temp dir");
    let shared_dir = temp_dir.path().join("shared");
    fs::create_dir(&shared_dir).unwrap();
    
    let dir_path = shared_dir.join("test_dir");
    fs::create_dir(&dir_path)?;
    
    let file_path = dir_path.join("test.txt");
    fs::write(&file_path, "Test")?;
    
    let entries: Vec<_> = fs::read_dir(&dir_path)?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    assert_eq!(entries.len(), 1);
    
    assert!(file_path.exists());
    
    fs::remove_file(&file_path)?;
    assert!(!file_path.exists());
    
    fs::remove_dir(&dir_path)?;
    assert!(!dir_path.exists());
    
    Ok(())
}

// ==================== ACL Tests ====================

#[test]
fn test_acl_operations() -> Result<()> {
    let (temp_dir, state) = create_test_fs_state();
    let shared_dir = temp_dir.path().join("shared");
    
    let file_path = shared_dir.join("acl_test.txt");
    fs::write(&file_path, "ACL test").unwrap();
    
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
    
    {
        let fs_state = state.read().unwrap();
        fs_state.set_acl(&file_path, acl.clone()).unwrap();
        
        let retrieved = fs_state.get_acl(&file_path);
        assert!(retrieved.is_some(), "ACL should be retrievable after setting");
        
        let retrieved_acl = retrieved.unwrap();
        assert_eq!(retrieved_acl.owner, 1000);
        assert_eq!(retrieved_acl.entries.len(), 2);
    }
    
    Ok(())
}

#[test]
fn test_acl_permission_check() -> Result<()> {
    let acl = Acl {
        owner: 1000,
        group: 1000,
        entries: vec![
            AclEntry {
                uid: 1000,
                gid: 1000,
                permissions: AclPermissions::READ | AclPermissions::WRITE,
            },
            AclEntry {
                uid: 2000,
                gid: 2000,
                permissions: AclPermissions::READ,
            },
        ],
    };
    
    // Owner should have read+write
    assert!(acl.check_permission(1000, 1000, AclPermissions::READ));
    assert!(acl.check_permission(1000, 1000, AclPermissions::WRITE));
    
    // Other user should only have read
    assert!(acl.check_permission(2000, 2000, AclPermissions::READ));
    assert!(!acl.check_permission(2000, 2000, AclPermissions::WRITE));
    
    // Unknown user should have no permissions
    assert!(!acl.check_permission(9999, 9999, AclPermissions::READ));
    
    Ok(())
}

// ==================== MMIO Tests ====================

#[test]
fn test_mmio_operations() -> Result<()> {
    let (temp_dir, state) = create_test_fs_state();
    
    // Create FsState with MMIO region
    let shared_dir = temp_dir.path().join("shared_mmio");
    fs::create_dir(&shared_dir).unwrap();
    let fs_state = FsState::new(&shared_dir, Some((0x1000, 0x1000))).unwrap();
    
    let test_data = [0x12, 0x34, 0x56, 0x78];
    let mut read_buf = [0u8; 4];
    
    // Write to MMIO
    fs_state.mmio_write(0x100, &test_data).unwrap();
    
    // Read from MMIO
    fs_state.mmio_read(0x100, &mut read_buf).unwrap();
    
    assert_eq!(test_data, read_buf, "MMIO read/write test failed");
    
    Ok(())
}

#[test]
fn test_mmio_out_of_bounds() -> Result<()> {
    let temp_dir = tempdir().expect("temp dir");
    let shared_dir = temp_dir.path().join("shared");
    fs::create_dir(&shared_dir).unwrap();
    let fs_state = FsState::new(&shared_dir, Some((0x1000, 0x100))).unwrap();
    
    let test_data = [0u8; 4];
    let mut read_buf = [0u8; 4];
    
    // Write past end should fail
    assert!(fs_state.mmio_write(0x100, &test_data).is_err());
    
    // Read past end should fail
    assert!(fs_state.mmio_read(0x100, &mut read_buf).is_err());
    
    Ok(())
}

// ==================== VirtioDevice Trait Tests ====================

#[test]
fn test_virtio_fs_device_type() -> Result<()> {
    let temp_dir = tempdir().expect("temp dir");
    let shared_dir = temp_dir.path().join("shared");
    fs::create_dir(&shared_dir).unwrap();
    
    let mem = GuestMemoryMmap::from_ranges(&[(GuestAddress(0), 0x10000)])
        .expect("Failed to create guest memory");
    let atomic_mem = vm_memory::atomic::GuestMemoryAtomic::new(mem);
    let irq = EventFd::new(0).expect("Failed to create EventFd");
    
    let fs = VirtioFs::new(&shared_dir, atomic_mem, irq)?;
    
    assert_eq!(fs.device_type(), 26, "VirtioFs device type should be 26");
    assert_eq!(fs.get_queues(), vec![256]);
    assert_eq!(fs.get_interrupt_status(), 0);
    
    Ok(())
}

// ==================== Concurrent Access Tests ====================

#[test]
fn test_concurrent_file_access() -> Result<()> {
    use std::thread;
    use std::sync::atomic::{AtomicUsize, Ordering};
    
    let temp_dir = tempdir().expect("temp dir");
    let shared_dir = temp_dir.path().join("shared");
    fs::create_dir(&shared_dir).unwrap();
    let shared_dir = Arc::new(shared_dir);
    
    let counter = Arc::new(AtomicUsize::new(0));
    let mut handles = vec![];
    
    for i in 0..5 {
        let shared_dir = shared_dir.clone();
        let counter = counter.clone();
        
        let handle = thread::spawn(move || {
            let file_path = shared_dir.join(format!("concurrent_{}.txt", i));
            let content = format!("Thread {}: {}", i, counter.fetch_add(1, Ordering::SeqCst));
            
            fs::write(&file_path, &content).unwrap();
            let read_content = fs::read_to_string(&file_path).unwrap();
            assert_eq!(read_content, content);
            
            fs::remove_file(&file_path).unwrap();
        });
        
        handles.push(handle);
    }
    
    for handle in handles {
        handle.join().unwrap();
    }
    
    assert_eq!(counter.load(Ordering::SeqCst), 5);
    
    Ok(())
}
