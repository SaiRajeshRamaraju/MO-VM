use kvm_bindings::kvm_userspace_memory_region;
use kvm_ioctls::Kvm;
use log::info;
use vm_memory::{GuestAddress, GuestMemoryMmap, MappedRegion};

use crate::error::{HypervisorError, Result};

const MEMORY_SIZE: u64 = 128 * 1024 * 1024; // 128MB of RAM
const KVM_32BIT_GAP_SIZE: u64 = 768 << 21;
const KVM_32BIT_GAP_START: u64 = 0x1_0000_0000; // 4GB

#[derive(Clone)]
pub struct GuestMemoryRegions {
    pub memory: GuestMemoryMmap,
    pub low_mem_size: u64,
}

impl GuestMemoryRegions {
    pub fn new() -> Result<Self> {
        // Create memory regions for the guest
        let low_mem_size = std::cmp::min(KVM_32BIT_GAP_START, MEMORY_SIZE);
        
        // Create guest memory
        let memory_regions = GuestMemoryMmap::from_ranges(&[
            (GuestAddress(0), low_mem_size as usize),
        ]).map_err(|e| HypervisorError::MemoryError(e.to_string()))?;
        
        // If we need more than 4GB, create a new memory region
        let memory = if MEMORY_SIZE > KVM_32BIT_GAP_START {
            let high_mem_size = MEMORY_SIZE - KVM_32BIT_GAP_START;
            GuestMemoryMmap::from_ranges(&[
                (GuestAddress(0), low_mem_size as usize),
                (GuestAddress(KVM_32BIT_GAP_START + KVM_32BIT_GAP_SIZE), high_mem_size as usize),
            ]).map_err(|e| HypervisorError::MemoryError(e.to_string()))?
        } else {
            memory_regions
        };
        
        Ok(Self {
            memory,
            low_mem_size,
        })
    }
    
    pub fn setup_memory_region(&self, _kvm: &Kvm, vm_fd: &kvm_ioctls::VmFd) -> Result<()> {
        // Set up the memory region with KVM
        // We assume a single memory region for simplicity
        let mem_region = kvm_userspace_memory_region {
            slot: 0,
            flags: 0,
            guest_phys_addr: 0,
            memory_size: self.low_mem_size,
            userspace_addr: 0, // Will be set by KVM
        };
        
        unsafe {
            vm_fd.set_user_memory_region(mem_region)
                .map_err(HypervisorError::KvmError)?;
        }
        
        info!("Initialized guest memory: {}MB", MEMORY_SIZE / (1024 * 1024));
        Ok(())
    }
}
