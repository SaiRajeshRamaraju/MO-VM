#!/bin/bash

set -e

# Create test directories
TEST_DIR="/tmp/virtio_fs_test"
MOUNT_POINT="$TEST_DIR/mnt"
SHARED_DIR="$TEST_DIR/shared"

# Clean up from previous runs
sudo umount -l "$MOUNT_POINT" 2>/dev/null || true
rm -rf "$TEST_DIR"
mkdir -p "$MOUNT_POINT" "$SHARED_DIR"

# Create test files
echo "Test file 1" > "$SHARED_DIR/file1.txt"
mkdir -p "$SHARED_DIR/test_dir"
echo "Nested file" > "$SHARED_DIR/test_dir/nested.txt"

# Mount the filesystem
# Note: Replace with your actual mount command
# sudo mount -t virtiofs myfs "$MOUNT_POINT" -o source=myfs

# Test 1: Basic file operations
echo "Testing basic file operations..."

# Read file
if [ "$(cat "$MOUNT_POINT/file1.txt")" != "Test file 1" ]; then
    echo "FAIL: File content mismatch"
    exit 1
fi

# Create new file
echo "New file content" > "$MOUNT_POINT/new_file.txt"
if [ ! -f "$MOUNT_POINT/new_file.txt" ]; then
    echo "FAIL: Failed to create new file"
    exit 1
fi

# Test 2: Directory operations
echo "Testing directory operations..."

# Create directory
mkdir "$MOUNT_POINT/new_dir"
if [ ! -d "$MOUNT_POINT/new_dir" ]; then
    echo "FAIL: Failed to create directory"
    exit 1
fi

# List directory
if [ "$(ls "$MOUNT_POINT" | wc -l)" -lt 2 ]; then
    echo "FAIL: Directory listing incorrect"
    exit 1
fi

# Test 3: Nested operations
echo "Testing nested operations..."

# Create nested directory
mkdir -p "$MOUNT_POINT/dir1/dir2"
echo "Nested content" > "$MOUNT_POINT/dir1/dir2/file.txt"

if [ "$(cat "$MOUNT_POINT/dir1/dir2/file.txt")" != "Nested content" ]; then
    echo "FAIL: Nested file content mismatch"
    exit 1
fi

# Test 4: File permissions
echo "Testing file permissions..."

# Test read permission
if ! [ -r "$MOUNT_POINT/file1.txt" ]; then
    echo "FAIL: File not readable"
    exit 1
fi

# Test write permission
if ! touch "$MOUNT_POINT/write_test" 2>/dev/null; then
    echo "WARNING: Write test failed - check permissions"
fi
rm -f "$MOUNT_POINT/write_test"

# Clean up
# sudo umount "$MOUNT_POINT"
# rm -rf "$TEST_DIR"

echo "All tests passed successfully!"
exit 0
