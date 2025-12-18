#!/bin/bash

# Create a test directory in the shared folder
mkdir -p /mnt/shared/test_dir

# Test file operations
echo "Testing file operations..."

# Write to a file
echo "This is a test file created from the guest" > /mnt/shared/test_dir/guest_file.txt

# Read from a file
echo "Contents of test_file.txt:"
cat /mnt/shared/test_file.txt

# List directory contents
echo "Listing /mnt/shared:"
ls -la /mnt/shared

# Test file permissions
echo "Testing file permissions..."
if [ -r /mnt/shared/test_file.txt ]; then
    echo "Read permission is working"
else
    echo "Read permission failed"
    exit 1
fi

# Test directory creation
if mkdir -p /mnt/shared/new_test_dir; then
    echo "Directory creation successful"
    # Clean up
    rmdir /mnt/shared/new_test_dir
else
    echo "Directory creation failed"
    exit 1
fi

echo "All tests completed successfully!"
exit 0
