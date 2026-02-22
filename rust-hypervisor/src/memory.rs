// =============================================================================
// memory.rs — Guest Physical Memory Management
// =============================================================================
//
// This module manages the guest VM's physical memory. In KVM-based virtualization,
// guest physical memory is actually host virtual memory that is mmap'd and then
// registered with KVM via the kvm_userspace_memory_region ioctl.
//
// MEMORY LAYOUT:
//   For memory <= 4GB (which is the default 128MB):
//     [0x0000_0000 ... memory_size)     ← Low memory (where kernel is loaded)
//
//   For memory > 4GB:
//     [0x0000_0000 ... 4GB)             ← Low memory
//     [4GB ... 4GB + PCI MMIO gap)      ← Reserved for PCI MMIO (no RAM here)
//     [4GB + gap ... end of memory)     ← High memory
//
// The PCI MMIO gap (768 * 2MB = 1.5GB starting at 4GB) is standard on x86
// and reserved for PCI device MMIO regions. RAM is placed below and above it.
//
// HOW KVM MEMORY SLOTS WORK:
//   - You mmap() a region of host memory
//   - You tell KVM: "guest physical address X maps to host address Y"
//   - When the guest accesses physical address X, KVM translates it to Y
//     using its internal page tables (EPT on Intel, NPT on AMD)
//   - Each mapping is called a "memory slot" (slot 0, slot 1, etc.)
// =============================================================================

use kvm_bindings::kvm_userspace_memory_region;
use kvm_ioctls::Kvm;
use log::info;
use vm_memory::{GuestAddress, GuestMemory, GuestMemoryRegion};
use vm_memory::mmap::GuestMemoryMmap;

use crate::error::{HypervisorError, Result};

/// Default guest memory size: 128MB
/// To change this, modify this constant and rebuild.
const MEMORY_SIZE: u64 = 128 * 1024 * 1024;

/// Size of the PCI MMIO gap (768 * 2MB = 1.5GB).
/// This region is reserved for PCI device MMIO and should not contain RAM.
const KVM_32BIT_GAP_SIZE: u64 = 768 << 21;

/// Start address of the PCI MMIO gap (4GB boundary).
const KVM_32BIT_GAP_START: u64 = 0x1_0000_0000;

// =============================================================================
// GuestMemoryRegions
// =============================================================================

/// Manages the guest's physical memory layout.
///
/// Wraps the `GuestMemoryMmap` from the vm-memory crate, which provides
/// the actual mmap'd host memory. Also tracks how much "low memory" we have
/// (below the PCI MMIO gap).
#[derive(Clone)]
pub struct GuestMemoryRegions {
    /// The underlying mmap'd memory (can contain multiple regions).
    pub memory: GuestMemoryMmap,
    /// Size of low memory in bytes (below the 4GB PCI MMIO gap).
    pub low_mem_size: u64,
}

impl GuestMemoryRegions {
    /// Creates a new guest memory layout.
    ///
    /// For our default 128MB configuration, this creates a single memory
    /// region starting at guest physical address 0x0.
    ///
    /// For >4GB configurations, it creates two regions:
    ///   - Region 0: [0, 4GB) — low memory
    ///   - Region 1: [4GB + gap, end) — high memory (above PCI MMIO gap)
    pub fn new() -> Result<Self> {
        // Low memory is capped at the PCI MMIO gap start (4GB)
        let low_mem_size = std::cmp::min(KVM_32BIT_GAP_START, MEMORY_SIZE);
        
        let memory = if MEMORY_SIZE > KVM_32BIT_GAP_START {
            // More than 4GB: split into low + high memory
            let high_mem_size = MEMORY_SIZE - KVM_32BIT_GAP_START;
            GuestMemoryMmap::from_ranges(&[
                // Low memory: 0 to 4GB
                (GuestAddress(0), low_mem_size as usize),
                // High memory: after the PCI MMIO gap
                (GuestAddress(KVM_32BIT_GAP_START + KVM_32BIT_GAP_SIZE), high_mem_size as usize),
            ]).map_err(|e| HypervisorError::MemoryError(e.to_string()))?
        } else {
            // 4GB or less: single contiguous region
            GuestMemoryMmap::from_ranges(&[
                (GuestAddress(0), low_mem_size as usize),
            ]).map_err(|e| HypervisorError::MemoryError(e.to_string()))?
        };
        
        Ok(Self {
            memory,
            low_mem_size,
        })
    }
    
    /// Registers all guest memory regions with KVM.
    ///
    /// For each memory region (there may be 1 or 2 depending on size):
    ///   1. Get the host virtual address from the mmap (via `as_ptr()`)
    ///   2. Create a `kvm_userspace_memory_region` struct that maps
    ///      guest physical address → host virtual address
    ///   3. Call the KVM `set_user_memory_region` ioctl to register it
    ///
    /// After this, any guest access to the registered physical addresses
    /// will be translated by hardware (EPT/NPT) to the host mmap'd memory.
    ///
    /// # Safety
    /// The `set_user_memory_region` ioctl is unsafe because:
    ///   - The kernel trusts that `userspace_addr` points to valid memory
    ///   - The memory must stay mapped for the lifetime of the VM
    ///   - Incorrect addresses can cause kernel panics or security issues
    pub fn setup_memory_region(&self, _kvm: &Kvm, vm_fd: &kvm_ioctls::VmFd) -> Result<()> {
        for (slot, region) in self.memory.iter().enumerate() {
            // Get the host-side pointer to this mmap'd region
            let host_addr = region.as_ptr() as u64;
            
            // Build the KVM memory slot descriptor
            let mem_region = kvm_userspace_memory_region {
                slot: slot as u32,                      // Slot index (0, 1, ...)
                flags: 0,                               // No special flags
                guest_phys_addr: region.start_addr().0,  // Guest physical address
                memory_size: region.len(),               // Size of the region
                userspace_addr: host_addr,               // Host virtual address (mmap'd)
            };
            
            // Register the memory slot with KVM
            unsafe {
                vm_fd.set_user_memory_region(mem_region)
                    .map_err(HypervisorError::KvmError)?;
            }
            
            info!(
                "Registered memory region slot={}: guest_addr=0x{:x}, size={}MB, host_addr=0x{:x}",
                slot,
                region.start_addr().0,
                region.len() / (1024 * 1024),
                host_addr,
            );
        }
        
        info!("Initialized guest memory: {}MB total", MEMORY_SIZE / (1024 * 1024));
        Ok(())
    }
}
