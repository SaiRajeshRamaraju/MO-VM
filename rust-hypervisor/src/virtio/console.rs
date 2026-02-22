// =============================================================================
// virtio/console.rs — Virtio Console Device
// =============================================================================
//
// This implements a virtual serial console following the virtio specification.
// The guest OS sees it as /dev/hvc0 (virtio console) and can write to it.
// Output from the guest is forwarded to the host's stdout.
//
// QUEUE LAYOUT:
//   Queue 0 = Receive (RX): Host → Guest (not yet implemented)
//   Queue 1 = Transmit (TX): Guest → Host (writes to stdout)
//
// When the guest writes to hvc0, the guest's virtio-console driver places
// the data into the TX queue. We read it here and write it to stdout.
// =============================================================================

use std::io::Write;
use std::sync::{Arc, Mutex};

use log::warn;
use vm_memory::{Bytes, GuestAddress, GuestMemoryMmap, Le16, Le32};
use virtio_queue::{QueueOwnedT, QueueT};
use crate::virtio::VirtioDeviceConfig;

use crate::error::{HypervisorError, Result};
use super::VirtioDevice;

/// Virtio device type ID for console devices (from the virtio spec)
const VIRTIO_ID_CONSOLE: u32 = 3;

// Feature bits
const VIRTIO_CONSOLE_F_SIZE: u64 = 0;       // Console reports terminal size
const VIRTIO_CONSOLE_F_MULTIPORT: u64 = 1;  // Console supports multiple ports

// ---- Console Configuration Space ----
// This is exposed to the guest via MMIO reads.
#[repr(C, packed)]
struct ConsoleConfig {
    cols: Le16,               // Terminal width in characters
    rows: Le16,               // Terminal height in characters
    max_nr_ports: Le32,       // Maximum number of console ports
    emerg_wr: Le32,           // Emergency write port (for panic output)
}

// =============================================================================
// VirtioConsole — Main Device Struct
// =============================================================================

/// A virtio console device that forwards guest output to a host writer.
pub struct VirtioConsole {
    /// Shared config: guest memory, queues, IRQ, interrupt eventfd
    config: VirtioDeviceConfig,
    /// Where to write guest console output (typically stdout).
    /// Arc<Mutex> because it could be shared with other threads.
    output: Arc<Mutex<dyn Write + Send>>,
    /// Configuration space that the guest can read
    config_space: ConsoleConfig,
}

impl VirtioConsole {
    /// Creates a new virtio console with 2 queues (RX and TX).
    ///
    /// The `output` parameter is where guest console output goes.
    /// Pass `Arc::new(Mutex::new(stdout()))` for normal use.
    pub fn new(
        mem: GuestMemoryMmap,
        guest_memory: GuestAddress,
        irq: u32,
        output: Arc<Mutex<dyn Write + Send>>,
    ) -> Result<Self> {
        // Console uses 2 queues: RX (index 0) and TX (index 1)
        let queues = 2;
        let config = VirtioDeviceConfig::new(mem, guest_memory, irq, queues)
            .map_err(|e| HypervisorError::MemoryError(e.to_string()))?;

        Ok(Self {
            config,
            output,
            config_space: ConsoleConfig {
                cols: Le16::from(80),      // 80 columns
                rows: Le16::from(24),      // 24 rows (standard terminal size)
                max_nr_ports: Le32::from(1), // Single port
                emerg_wr: Le32::from(0),
            },
        })
    }

    /// Processes the receive queue (host → guest).
    /// Not yet implemented — would read from host stdin and push to guest.
    fn process_rx_queue(&mut self) -> Result<()> {
        // TODO: Read from stdin and write to guest's RX queue
        Ok(())
    }

    /// Processes the transmit queue (guest → host).
    ///
    /// Reads each descriptor chain from the TX queue, extracts the data
    /// bytes from guest memory, and writes them to the host output.
    fn process_tx_queue(&mut self) -> Result<()> {
        let mem = &self.config.mem;
        // Queue index 1 is the TX queue
        let queue = self.config.queues.get_mut(1).unwrap();
        let mut used_any = false;
        let mut output = self.output.lock().unwrap();

        // Iterate over all available descriptor chains in the TX queue
        while let Some(desc_chain) = queue.iter(mem).map_err(|e| {
            HypervisorError::MemoryError(format!("Failed to iterate queue: {}", e))
        })?.next() {
            used_any = true;
            let head_index = desc_chain.head_index();

            let mut buf = Vec::new();
            // Walk the descriptor chain and collect all data
            for desc in desc_chain {
                // Skip write-only descriptors (those are for host→guest direction)
                if !desc.is_write_only() {
                    let len = desc.len() as usize;
                    let mut data = vec![0u8; len];
                    // Read the data from guest memory at the descriptor's address
                    let _ = mem.read_slice(&mut data[..], desc.addr());
                    buf.extend_from_slice(&data);
                }
            }

            // Write the collected data to host stdout
            if !buf.is_empty() {
                let _ = output.write_all(&buf);
            }

            // Mark descriptor chain as used (completed)
            let _ = queue.add_used(mem, head_index, buf.len() as u32);
        }

        // If we processed any output, signal an interrupt to the guest
        if used_any {
            if let Some(evt) = &self.config.interrupt_evt {
                evt.write(1u64).map_err(|e| {
                    HypervisorError::IoError(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!("Failed to signal interrupt: {}", e),
                    ))
                })?;
            }
        }

        Ok(())
    }
}

// =============================================================================
// VirtioDevice Trait Implementation
// =============================================================================

impl VirtioDevice for VirtioConsole {
    type Error = crate::error::HypervisorError;

    fn device_type(&self) -> u32 { VIRTIO_ID_CONSOLE }

    /// We only advertise VIRTIO_CONSOLE_F_SIZE (terminal dimensions)
    fn get_features(&self) -> u64 {
        1 << VIRTIO_CONSOLE_F_SIZE
    }

    fn set_acked_features(&mut self, features: u64) -> Result<()> {
        if (features & !self.get_features()) != 0 {
            warn!("Guest tried to enable unsupported features: {:#x}", features);
        }
        Ok(())
    }

    /// Reads the console configuration space (terminal size, port count)
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

    fn write_config(&mut self, _offset: u64, _data: &[u8]) -> Result<()> { Ok(()) }

    /// Routes queue processing to the appropriate handler by index
    fn process_queue(&mut self, queue_idx: u32) -> Result<()> {
        match queue_idx {
            0 => self.process_rx_queue(),   // RX: host → guest (not yet implemented)
            1 => self.process_tx_queue(),   // TX: guest → host (prints to stdout)
            _ => Err(HypervisorError::MemoryError("Invalid queue index".to_string())),
        }
    }

    fn get_queues(&self) -> Vec<u16> {
        self.config.queues().iter().map(|q| q.max_size()).collect()
    }

    fn get_interrupt_status(&self) -> u32 { 0 }
}
