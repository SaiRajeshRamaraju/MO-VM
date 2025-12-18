use std::io::Write;
use std::sync::{Arc, Mutex};

use log::warn;
use vm_memory::{Bytes, GuestAddress, GuestMemoryMmap, Le16, Le32};
use vm_virtio::queue::Queue;
use vm_virtio::VirtioDeviceConfig;

use crate::error::{HypervisorError, Result};
use super::VirtioDevice;

// Virtio console device ID
const VIRTIO_ID_CONSOLE: u32 = 3;

// Feature bits
const VIRTIO_CONSOLE_F_SIZE: u64 = 0; // Console size is available
const VIRTIO_CONSOLE_F_MULTIPORT: u64 = 1; // Multiple ports are available

// Console configuration layout
#[repr(C, packed)]
struct ConsoleConfig {
    cols: Le16,
    rows: Le16,
    max_nr_ports: Le32,
    emerg_wr: Le32,
}

pub struct VirtioConsole {
    config: VirtioDeviceConfig<Queue<GuestMemoryMmap>>,
    output: Arc<Mutex<dyn Write + Send>>,
    config_space: ConsoleConfig,
}

impl VirtioConsole {
    pub fn new(
        mem: GuestMemoryMmap,
        guest_memory: GuestAddress,
        irq: u32,
        output: Arc<Mutex<dyn Write + Send>>,
    ) -> Result<Self> {
        let queues = 2; // One for receive, one for transmit
        let config = VirtioDeviceConfig::new(mem, guest_memory, irq, queues as u16)
            .map_err(|e| HypervisorError::MemoryError(e.to_string()))?;

        Ok(Self {
            config,
            output,
            config_space: ConsoleConfig {
                cols: Le16::from(80),
                rows: Le16::from(24),
                max_nr_ports: Le32::from(1),
                emerg_wr: Le32::from(0),
            },
        })
    }

    fn process_rx_queue(&mut self) -> Result<()> {
        // For console, RX queue is for host to send data to guest
        // We don't support sending data from host to guest in this simple implementation
        Ok(())
    }

    fn process_tx_queue(&mut self) -> Result<()> {
        let mem = self.config.memory();
        let mut queue = self.config.queues_mut().get_mut(1).unwrap(); // TX queue is index 1
        let mut used_any = false;
        let mut output = self.output.lock().unwrap();

        while let Some(mut chain) = queue.iter(mem).map_err(|e| {
            HypervisorError::MemoryError(format!("Failed to iterate queue: {}", e))
        })?.next() {
            used_any = true;
            
            // Read data from the guest
            let mut buf = Vec::new();
            for desc_chain in chain.iter() {
                if !desc_chain.is_write_only() {
                    let len = desc_chain.len() as usize;
                    let mut data = vec![0u8; len];
                    desc_chain.read_exact(&mut data[..])?;
                    buf.extend_from_slice(&data);
                }
            }
            
            // Write to output
            if !buf.is_empty() {
                output.write_all(&buf).map_err(|e| {
                    HypervisorError::IoError(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!("Failed to write to console: {}", e),
                    ))
                })?;
            }
            
            // Mark the descriptor as used
            queue.add_used(mem, chain.head_index(), buf.len() as u32)
                .map_err(|e| {
                    HypervisorError::MemoryError(format!("Failed to add used descriptor: {}", e))
                })?;
        }
        
        if used_any {
            self.config.interrupt_evt.write(1).map_err(|e| {
                HypervisorError::IoError(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("Failed to signal interrupt: {}", e),
                ))
            })?;
        }
        
        Ok(())
    }
}

impl VirtioDevice for VirtioConsole {
    type Error = crate::error::Error;
    fn device_type(&self) -> u32 {
        VIRTIO_ID_CONSOLE
    }
    
    fn get_features(&self) -> u64 {
        // We only support basic console features
        1 << VIRTIO_CONSOLE_F_SIZE
    }
    
    fn set_acked_features(&mut self, features: u64) -> Result<()> {
        // In a real implementation, we would validate the features here
        if (features & !self.get_features()) != 0 {
            warn!("Guest tried to enable unsupported features: {:#x}", features);
        }
        Ok(())
    }
    
    fn read_config(&self, offset: u64, data: &mut [u8]) -> Result<()> {
        let config_slice = unsafe {
            std::slice::from_raw_parts(
                &self.config_space as *const _ as *const u8,
                std::mem::size_of::<ConsoleConfig>(),
            )
        };
        
        let config_len = config_slice.len() as u64;
        if offset >= config_len {
            return Err(HypervisorError::MemoryError(
                "Invalid config space offset".to_string(),
            ));
        }
        
        let len = std::cmp::min(data.len() as u64, config_len - offset) as usize;
        data[..len].copy_from_slice(&config_slice[offset as usize..(offset as usize + len)]);
        
        Ok(())
    }
    
    fn write_config(&mut self, _offset: u64, _data: &[u8]) -> Result<()> {
        // Console config is read-only in this implementation
        Ok(())
    }
    
    fn process_queue(&mut self, queue_idx: u32) -> Result<()> {
        match queue_idx {
            0 => self.process_rx_queue(), // RX queue
            1 => self.process_tx_queue(), // TX queue
            _ => Err(HypervisorError::MemoryError(
                "Invalid queue index".to_string(),
            )),
        }
    }
    
    fn get_queues(&self) -> Vec<u16> {
        self.config.queues().iter().map(|q| q.size).collect()
    }
    
    fn get_interrupt_status(&self) -> u32 {
        // In a real implementation, we would track interrupt status
        0
    }
}
