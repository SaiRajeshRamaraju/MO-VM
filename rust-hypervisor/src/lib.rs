//! Rust Hypervisor - A minimal hypervisor implementation in Rust

#![deny(missing_docs)]
#![deny(warnings)]

pub mod error;
pub mod kernel_loader;
pub mod memory;
pub mod vcpu;
pub mod virtio;
pub mod vm;
pub mod snapshot;

// Re-exports for convenience
pub use error::Error;
pub use vm::VirtualMachine;
pub use virtio::fs as virtio_fs;
pub use virtio_fs::{VirtioFs, Acl, AclEntry, AclPermissions};
