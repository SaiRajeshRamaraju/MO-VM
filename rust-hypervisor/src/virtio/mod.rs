pub mod block;
pub mod console;
pub mod fs;
pub mod net;

pub use block::VirtioBlock;
pub use console::VirtioConsole;
pub use net::VirtioNet;
pub use fs::{VirtioFs, Error as FsError, Acl, AclEntry, AclPermissions};

/// Base trait for all virtio devices
pub trait VirtioDevice: Send + Sync {
    /// The error type for this device
    type Error: std::error::Error + Send + Sync + 'static;
    
    /// Device type as defined in the virtio specification
    fn device_type(&self) -> u32;
    
    /// Get the available features of this device
    fn get_features(&self) -> u64;
    
    /// Set the features that have been negotiated
    fn set_acked_features(&mut self, features: u64) -> Result<(), Self::Error>;
    
    /// Read from the device's configuration space
    fn read_config(&self, offset: u64, data: &mut [u8]) -> Result<(), Self::Error>;
    
    /// Write to the device's configuration space
    fn write_config(&mut self, offset: u64, data: &[u8]) -> Result<(), Self::Error>;
    
    /// Process a virtqueue
    fn process_queue(&mut self, queue_idx: u32) -> Result<(), Self::Error>;
    
    /// Get the list of queue sizes for the device
    fn get_queues(&self) -> Vec<u16>;
    
    /// Get the device's interrupt status
    fn get_interrupt_status(&self) -> u32;
    
    /// Gracefully shutdown the device
    fn shutdown(&mut self) -> Result<(), Self::Error> {
        // Default implementation does nothing
        Ok(())
    }
    
    /// Handle MMIO read
    fn mmio_read(&self, _offset: u64, _data: &mut [u8]) -> Result<(), Self::Error> {
        Ok(())
    }
    
    /// Handle MMIO write
    fn mmio_write(&mut self, _offset: u64, _data: &[u8]) -> Result<()> {
        Ok(())
    }
    
    /// Get the MMIO region if the device has one
    fn get_mmio_region(&self) -> Option<(u64, u64)> {
        None
    }
}
