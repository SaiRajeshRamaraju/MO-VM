//! Virtio device abstractions and implementations.
//!
//! This module provides the `VirtioDevice` trait and implementations for:
//! - Block devices (`virtio-blk`)
//! - Console devices (`virtio-console`)
//! - Network devices (`virtio-net`)
//! - Filesystem devices (`virtio-fs`)

/// Virtio block device implementation.
pub mod block;
/// Virtio console device implementation.
pub mod console;
/// Virtio filesystem (9P) device implementation.
pub mod fs;
/// Virtio network device implementation.
pub mod net;

pub use block::VirtioBlock;
pub use console::VirtioConsole;
pub use net::VirtioNet;
pub use fs::{VirtioFs, FsState, Error as FsError, Acl, AclEntry, AclPermissions};

/// Base trait for all virtio devices.
///
/// Implementors must provide device identification, feature negotiation,
/// configuration space access, and queue processing.
pub trait VirtioDevice: Send + Sync {
    /// The error type for this device.
    type Error: std::error::Error + Send + Sync + 'static;
    
    /// Device type as defined in the virtio specification.
    fn device_type(&self) -> u32;
    
    /// Get the available features of this device.
    fn get_features(&self) -> u64;
    
    /// Set the features that have been negotiated with the guest.
    fn set_acked_features(&mut self, features: u64) -> Result<(), Self::Error>;
    
    /// Read from the device's configuration space.
    fn read_config(&self, offset: u64, data: &mut [u8]) -> Result<(), Self::Error>;
    
    /// Write to the device's configuration space.
    fn write_config(&mut self, offset: u64, data: &[u8]) -> Result<(), Self::Error>;
    
    /// Process a virtqueue by index.
    fn process_queue(&mut self, queue_idx: u32) -> Result<(), Self::Error>;
    
    /// Get the list of queue sizes for the device.
    fn get_queues(&self) -> Vec<u16>;
    
    /// Get the device's interrupt status.
    fn get_interrupt_status(&self) -> u32;
    
    /// Gracefully shutdown the device.
    fn shutdown(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
    
    /// Handle an MMIO read at the given offset.
    fn mmio_read(&self, _offset: u64, _data: &mut [u8]) -> Result<(), Self::Error> {
        Ok(())
    }
    
    /// Handle an MMIO write at the given offset.
    fn mmio_write(&mut self, _offset: u64, _data: &[u8]) -> Result<(), Self::Error> {
        Ok(())
    }
    
    /// Get the MMIO region (base, size) if the device has one.
    fn get_mmio_region(&self) -> Option<(u64, u64)> {
        None
    }
}

use virtio_queue::QueueT;

/// Configuration and state for a virtio device.
///
/// Holds guest memory, queues, IRQ number, and the interrupt event fd.
pub struct VirtioDeviceConfig<Q = virtio_queue::Queue> {
    /// Guest physical memory.
    pub mem: vm_memory::GuestMemoryMmap,
    /// Base guest address for this device.
    pub guest_mem: vm_memory::GuestAddress,
    /// IRQ number assigned to this device.
    pub irq: u32,
    /// Virtio queues for this device.
    pub queues: Vec<Q>,
    /// Event fd used to signal interrupts to the guest.
    pub interrupt_evt: Option<vmm_sys_util::eventfd::EventFd>,
}

impl VirtioDeviceConfig<virtio_queue::Queue> {
    /// Creates a new device configuration with the given parameters.
    pub fn new(
        mem: vm_memory::GuestMemoryMmap,
        guest_mem: vm_memory::GuestAddress,
        irq: u32,
        num_queues: usize,
    ) -> Result<Self, String> {
        let mut queues = Vec::new();
        for _ in 0..num_queues {
            queues.push(virtio_queue::Queue::new(1024).unwrap());
        }
        let interrupt_evt = vmm_sys_util::eventfd::EventFd::new(libc::EFD_NONBLOCK).ok();
        Ok(Self { mem, guest_mem, irq, queues, interrupt_evt })
    }

    /// Returns a reference to the guest memory.
    pub fn memory(&self) -> &vm_memory::GuestMemoryMmap { &self.mem }

    /// Returns a slice of the device's queues.
    pub fn queues(&self) -> &[virtio_queue::Queue] { &self.queues }

    /// Returns a mutable slice of the device's queues.
    pub fn queues_mut(&mut self) -> &mut [virtio_queue::Queue] { &mut self.queues }
}
