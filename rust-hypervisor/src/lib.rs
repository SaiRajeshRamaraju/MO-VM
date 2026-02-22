// =============================================================================
// lib.rs — Crate Root (Public API)
// =============================================================================
//
// This is the library crate for the Rust hypervisor. It declares all modules
// and re-exports the key types that users of the crate need.
//
// ARCHITECTURE OVERVIEW:
//   ┌──────────────────────────────────────────────────┐
//   │  main.rs (binary) — CLI parsing & orchestration  │
//   └──────────────────┬───────────────────────────────┘
//                      │ uses
//   ┌──────────────────▼───────────────────────────────┐
//   │  lib.rs — public API of the crate                │
//   ├──────────┬───────────┬───────────┬───────────────┤
//   │ vm.rs    │ vcpu.rs   │memory.rs  │kernel_loader  │
//   │ (VM      │ (vCPU     │(guest     │(ELF loader)   │
//   │ mgmt)    │ init/run) │ RAM)      │               │
//   ├──────────┴───────────┼───────────┴───────────────┤
//   │ virtio/              │ snapshot.rs               │
//   │ ├── mod.rs (trait)   │ (save/restore VM state)   │
//   │ ├── block.rs         │                           │
//   │ ├── console.rs       │                           │
//   │ ├── net.rs           │                           │
//   │ └── fs.rs            │                           │
//   ├──────────────────────┴───────────────────────────┤
//   │ error.rs — unified error type & Result alias     │
//   └──────────────────────────────────────────────────┘
//
// QUICK START:
//   use rust_hypervisor::VirtualMachine;
//   let mut vm = VirtualMachine::new(1)?;   // 1 vCPU
//   vm.add_virtio_console()?;
//   vm.load_kernel(Path::new("vmlinuz"))?;
//   vm.run()?;
// =============================================================================

//! # Rust Hypervisor
//!
//! A minimal KVM-based hypervisor implementation in Rust supporting:
//! - x86_64 guest virtual machines
//! - Virtio devices (block, console, network, filesystem)
//! - VM state snapshot and restore
//! - Multi-vCPU support with scoped threads

// Warn (but don't error) on missing doc comments — allows incremental docs
#![warn(missing_docs)]

/// Error types and the `Result<T>` alias used throughout the crate.
pub mod error;

/// ELF kernel loader for loading Linux kernels into guest memory.
/// Parses ELF headers and writes PT_LOAD segments into guest RAM.
#[allow(dead_code)]
pub mod kernel_loader;

/// Guest physical memory management.
/// Handles mmap allocation and KVM memory slot registration.
pub mod memory;

/// Virtual CPU management — register init, KVM_RUN loop, MSR setup.
pub mod vcpu;

/// Virtio device implementations (block, console, net, filesystem).
/// Contains the `VirtioDevice` trait and concrete implementations.
#[allow(dead_code)]
pub mod virtio;

/// Virtual Machine lifecycle management.
/// The `VirtualMachine` struct orchestrates KVM, vCPUs, devices, and snapshots.
pub mod vm;

/// VM state snapshot and restore functionality.
/// Custom serde impls for KVM register structures (kvm_regs, kvm_sregs, etc.)
#[allow(dead_code)]
pub mod snapshot;

// --- Convenience Re-exports ---
// These let users write `rust_hypervisor::VirtualMachine` instead of
// `rust_hypervisor::vm::VirtualMachine`.

/// The main error type.
pub use error::HypervisorError;
/// The main VM struct.
pub use vm::VirtualMachine;
/// Virtio filesystem types (for direct access to VirtioFs, ACLs, etc.)
pub use virtio::fs as virtio_fs;
/// Re-exported VirtioFs types.
pub use virtio_fs::{VirtioFs, FsState, Acl, AclEntry, AclPermissions};
