use crate::error::{HypervisorError, Result};
use crate::virtio::VirtioDevice;
use log::warn;
use std::sync::{Arc, Mutex};

pub struct VirtioMmioDevice {
    pub device: Arc<Mutex<Box<dyn VirtioDevice<Error = HypervisorError>>>>,
    pub base_addr: u64,
    pub size: u64,
    queue_sel: u32,
}

impl VirtioMmioDevice {
    pub fn new(device: Box<dyn VirtioDevice<Error = HypervisorError>>, base_addr: u64) -> Self {
        Self {
            device: Arc::new(Mutex::new(device)),
            base_addr,
            size: 0x1000,
            queue_sel: 0,
        }
    }

    pub fn mmio_read(&mut self, offset: u64, data: &mut [u8]) -> Result<()> {
        let mut dev = self.device.lock().unwrap();
        if offset >= 0x100 {
            return dev.read_config(offset - 0x100, data);
        }
        
        let mut val = 0u32;
        match offset {
            0x00 => val = 0x74726976, // "virt"
            0x04 => val = 1, // version
            0x08 => val = dev.device_type(),
            0x0c => val = 0x4d564f4d, // "MOVM"
            0x10 => val = (dev.get_features() & 0xffffffff) as u32,
            0x14 => val = (dev.get_features() >> 32) as u32, // Host features high
            0x34 => { // QueueNumMax
                let queues = dev.get_queues();
                if (self.queue_sel as usize) < queues.len() {
                    val = queues[self.queue_sel as usize] as u32;
                } else {
                    val = 0;
                }
            }
            0x44 => val = 1, // QueueReady
            0x60 => val = dev.get_interrupt_status(),
            0x70 => val = 0, // Status
            _ => { warn!("Unhandled MMIO read offset: 0x{:x}", offset); }
        }
        
        let bytes = val.to_le_bytes();
        let len = std::cmp::min(data.len(), bytes.len());
        data[..len].copy_from_slice(&bytes[..len]);
        Ok(())
    }

    pub fn mmio_write(&mut self, offset: u64, data: &[u8]) -> Result<()> {
        let mut dev = self.device.lock().unwrap();
        if offset >= 0x100 {
            return dev.write_config(offset - 0x100, data);
        }
        
        let mut val_bytes = [0u8; 4];
        let len = std::cmp::min(data.len(), 4);
        val_bytes[..len].copy_from_slice(&data[..len]);
        let val = u32::from_le_bytes(val_bytes);

        match offset {
            0x20 => { // GuestFeatures
                // We just ack what they give for now
                let _ = dev.set_acked_features(val as u64);
            }
            0x30 => self.queue_sel = val, // QueueSel
            0x50 => { // QueueNotify
                let _ = dev.process_queue(val);
            }
            // For a complete transport, we'd also handle writing QueuePFN (0x40) or QueueDescLow (0x80)
            _ => { warn!("Unhandled MMIO write offset: 0x{:x} val: 0x{:x}", offset, val); }
        }
        Ok(())
    }
}
