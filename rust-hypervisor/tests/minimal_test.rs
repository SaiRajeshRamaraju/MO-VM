use std::fs;
use std::path::Path;
use tempfile::tempdir;
use vm_memory::{GuestAddress, GuestMemoryMmap};

#[test]
fn test_minimal() -> std::result::Result<(), Box<dyn std::error::Error>> {
    // Create a temporary directory
    let temp_dir = tempdir()?;
    let shared_dir = temp_dir.path().join("shared");
    fs::create_dir(&shared_dir)?;

    // Create a test file
    let test_file = shared_dir.join("test.txt");
    fs::write(&test_file, "Hello, World!")?;

    // Read the file back
    let content = fs::read_to_string(&test_file)?;
    assert_eq!(content, "Hello, World!");

    // Test directory listing
    let entries: Vec<_> = fs::read_dir(&shared_dir)?.collect::<Result<Vec<_>, _>>()?;
    assert!(!entries.is_empty(), "Should have at least one entry");

    // Clean up
    fs::remove_file(&test_file)?;
    fs::remove_dir(&shared_dir)?;

    Ok(())
}
