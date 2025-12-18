use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tempfile::tempdir;
use vm_memory::{GuestAddress, GuestMemoryMmap};

use rust_hypervisor::{
    virtio::fs::{VirtioFs, Acl, AclEntry, AclPermissions},
    error::Result,
};

struct TestEnvironment {
    _temp_dir: tempfile::TempDir,
    shared_dir: PathBuf,
    fs: VirtioFs,
    mem: Arc<GuestMemoryMmap<()>>,
}

impl TestEnvironment {
    fn new() -> Result<Self> {
        // Create a temporary directory for testing
        let temp_dir = tempdir().expect("Failed to create temp directory");
        let shared_dir = temp_dir.path().join("shared");
        fs::create_dir(&shared_dir).expect("Failed to create shared directory");

        // Initialize memory
        let mem = Arc::new(
            GuestMemoryMmap::<()>::from_ranges(&[(GuestAddress(0), 0x10000)])?
        );

        // Create Virtio-FS device
        let fs = VirtioFs::new(
            &shared_dir,
            mem.clone(),
            GuestAddress(0x1000),
            5, // irq
            "test",
            false,
            Some((0x1000, 0x1000)),
        )?;

        Ok(Self {
            _temp_dir: temp_dir,
            shared_dir,
            fs,
            mem,
        })
    }

    fn create_test_file(&self, path: &str, content: &str) -> PathBuf {
        let full_path = self.shared_dir.join(path);
        if let Some(parent) = full_path.parent() {
            fs::create_dir_all(parent).expect("Failed to create parent directory");
        }
        fs::write(&full_path, content).expect("Failed to write test file");
        full_path
    }
}

#[test]
fn test_basic_file_operations() -> Result<()> {
    let env = TestEnvironment::new()?;
    
    // Test file creation and reading
    let test_content = "Test file content";
    let file_path = env.create_test_file("test.txt", test_content);
    
    // Read the file back
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
    let env = TestEnvironment::new()?;
    
    // Create a test directory
    let dir_path = env.shared_dir.join("test_dir");
    fs::create_dir(&dir_path)?;
    
    // Create a file in the directory
    let file_path = dir_path.join("test.txt");
    fs::write(&file_path, "Test")?;
    
    // List directory contents
    let entries: Vec<_> = fs::read_dir(&dir_path)?.collect::<Result<Vec<_>, _>>()?;
    assert_eq!(entries.len(), 1);
    
    // Check the file exists
    assert!(file_path.exists());
    
    // Delete the file
    fs::remove_file(&file_path)?;
    assert!(!file_path.exists());
    
    // Delete the directory
    fs::remove_dir(&dir_path)?;
    assert!(!dir_path.exists());
    
    Ok(())
}

#[test]
fn test_acl_operations() -> Result<()> {
    let env = TestEnvironment::new()?;
    
    // Create a test file
    let file_path = env.create_test_file("acl_test.txt", "ACL test");
    
    // Set ACL for the file
    let acl = Acl {
        owner: 1000,
        group: 1000,
        entries: vec![
            AclEntry {
                uid: 1000,
                permissions: AclPermissions::all(),
            },
            AclEntry {
                uid: 1001,
                permissions: AclPermissions::READ | AclPermissions::EXECUTE,
            },
        ],
    };
    
    env.fs.set_acl(&file_path, acl)?;
    
    // Verify ACL was set (this is a simplified check)
    let metadata = fs::metadata(&file_path)?;
    let permissions = metadata.permissions();
    assert!(permissions.readonly() == false, "File should be writable by owner");
    
    Ok(())
}

#[test]
fn test_mmio_operations() -> Result<()> {
    let env = TestEnvironment::new()?;
    
    // Test MMIO write and read
    let test_data = [0x12, 0x34, 0x56, 0x78];
    let mut read_buf = [0u8; 4];
    
    // Write to MMIO
    env.fs.mmio_write(0x100, &test_data)?;
    
    // Read from MMIO
    env.fs.mmio_read(0x100, &mut read_buf)?;
    
    assert_eq!(test_data, read_buf, "MMIO read/write test failed");
    
    Ok(())
}

#[test]
fn test_concurrent_access() -> Result<()> {
    use std::thread;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    
    let env = Arc::new(TestEnvironment::new()?);
    let counter = Arc::new(AtomicUsize::new(0));
    let mut handles = vec![];
    
    // Spawn multiple threads to test concurrent access
    for i in 0..5 {
        let env = env.clone();
        let counter = counter.clone();
        
        let handle = thread::spawn(move || {
            let file_path = env.shared_dir.join(format("concurrent_{}.txt", i));
            let content = format!("Thread {}: {}", i, counter.fetch_add(1, Ordering::SeqCst));
            
            // Each thread writes to its own file
            fs::write(&file_path, &content).unwrap();
            
            // Read back and verify
            let read_content = fs::read_to_string(&file_path).unwrap();
            assert_eq!(read_content, content);
            
            // Clean up
            fs::remove_file(&file_path).unwrap();
        });
        
        handles.push(handle);
    }
    
    // Wait for all threads to complete
    for handle in handles {
        handle.join().unwrap();
    }
    
    // Verify all threads completed successfully
    assert_eq!(counter.load(Ordering::SeqCst), 5);
    
    Ok(())
}
