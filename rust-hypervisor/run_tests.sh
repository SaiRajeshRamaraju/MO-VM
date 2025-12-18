#!/bin/bash
set -e

echo "=== Cleaning previous build ==="
cargo clean

echo -e "\n=== Building the project ==="
cargo build

echo -e "\n=== Running unit tests ==="
cargo test --lib -- --nocapture

echo -e "\n=== Running integration tests ==="
cargo test --test integration_test -- --nocapture

echo -e "\n=== Running filesystem operations test ==="
chmod +x tests/fs_operations_test.sh
./tests/fs_operations_test.sh

echo -e "\n=== All tests completed successfully ==="
