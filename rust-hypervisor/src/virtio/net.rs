use std::io;
use std::net::UdpSocket;
use std::sync::{Arc, Mutex};

use log::{error, info, warn};
use vm_memory::{Bytes, GuestAddress, GuestMemoryMmap};
use virtio_queue::{QueueOwnedT, QueueT};
use crate::virtio::VirtioDeviceConfig;

use crate::error::{HypervisorError, Result};
use super::VirtioDevice;

// Virtio network device ID
const VIRTIO_ID_NET: u32 = 1;

// Feature bits
const VIRTIO_NET_F_MAC: u64 = 5;
const VIRTIO_NET_F_STATUS: u64 = 16;
const VIRTIO_F_VERSION_1: u64 = 1 << 32;

// Network device configuration
#[repr(C, packed)]
struct NetConfig {
    mac: [u8; 6],
    status: u16,
    max_virtqueue_pairs: u16,
    mtu: u16,
}

pub struct VirtioNet {
    config: VirtioDeviceConfig,
    socket: Arc<Mutex<UdpSocket>>,
    local_mac: [u8; 6],
    peer_addr: Option<std::net::SocketAddr>,
}

impl VirtioNet {
    pub fn new(
        mem: GuestMemoryMmap,
        guest_mem: GuestAddress,
        irq: u32,
        local_addr: &str,
        peer_addr: Option<&str>,
    ) -> Result<Self> {
        let socket = UdpSocket::bind(local_addr).map_err(|e| {
            HypervisorError::IoError(io::Error::new(
                io::ErrorKind::Other,
                format!("Failed to bind socket: {}", e),
            ))
        })?;

        socket.set_nonblocking(true).map_err(|e| {
            HypervisorError::IoError(io::Error::new(
                io::ErrorKind::Other,
                format!("Failed to set non-blocking: {}", e),
            ))
        })?;

        let peer_socket = peer_addr.map(|addr| {
            addr.parse().map_err(|e| {
                HypervisorError::IoError(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("Invalid peer address: {}", e),
                ))
            })
        }).transpose()?;

        let config = VirtioDeviceConfig::new(mem, guest_mem, irq, 2).map_err(|e| {
            HypervisorError::MemoryError(format!("Failed to create virtio config: {}", e))
        })?;

        let mut mac = [0u8; 6];
        getrandom::getrandom(&mut mac).map_err(|e| {
            HypervisorError::IoError(io::Error::new(
                io::ErrorKind::Other,
                format!("Failed to generate MAC address: {}", e),
            ))
        })?;
        mac[0] = 0x02;

        Ok(Self {
            config,
            socket: Arc::new(Mutex::new(socket)),
            local_mac: mac,
            peer_addr: peer_socket,
        })
    }

    fn process_rx_queue(&mut self) -> Result<()> {
        let mem = &self.config.mem;
        let queue = self.config.queues.get_mut(0).ok_or_else(|| {
            HypervisorError::MemoryError("RX queue not found".into())
        })?;

        let socket = self.socket.lock().map_err(|_| {
            HypervisorError::IoError(io::Error::new(
                io::ErrorKind::Other,
                "Failed to lock socket".to_string(),
            ))
        })?;

        while let Some(desc_chain) = queue.iter(mem).map_err(|e| {
            HypervisorError::MemoryError(format!("Failed to iterate queue: {}", e))
        })?.next() {
            let head_index = desc_chain.head_index();
            let mut buf = vec![0u8; 1514];

            match socket.recv_from(&mut buf) {
                Ok((len, _)) => {
                    info!("Received {} bytes on network interface", len);
                }
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                    break;
                }
                Err(e) => {
                    return Err(HypervisorError::IoError(e));
                }
            }

            let _ = queue.add_used(mem, head_index, 0);
        }

        Ok(())
    }

    fn process_tx_queue(&mut self) -> Result<()> {
        let mem = &self.config.mem;
        let queue = self.config.queues.get_mut(1).ok_or_else(|| {
            HypervisorError::MemoryError("TX queue not found".into())
        })?;

        let socket = self.socket.lock().map_err(|_| {
            HypervisorError::IoError(io::Error::new(
                io::ErrorKind::Other,
                "Failed to lock socket".to_string(),
            ))
        })?;

        while let Some(desc_chain) = queue.iter(mem).map_err(|e| {
            HypervisorError::MemoryError(format!("Failed to iterate queue: {}", e))
        })?.next() {
            let head_index = desc_chain.head_index();
            let mut buf = Vec::new();

            for desc in desc_chain {
                let mut chunk = vec![0u8; desc.len() as usize];
                let _ = mem.read_slice(&mut chunk, desc.addr());
                buf.extend_from_slice(&chunk);
            }

            if let Some(peer) = self.peer_addr {
                if let Err(e) = socket.send_to(&buf, peer) {
                    error!("Failed to send packet: {}", e);
                }
            }

            let _ = queue.add_used(mem, head_index, buf.len() as u32);
        }

        Ok(())
    }
}

impl VirtioDevice for VirtioNet {
    type Error = crate::error::HypervisorError;
    fn device_type(&self) -> u32 {
        VIRTIO_ID_NET
    }

    fn get_features(&self) -> u64 {
        (1u64.wrapping_shl(VIRTIO_F_VERSION_1 as u32))
            | (1u64.wrapping_shl(VIRTIO_NET_F_MAC as u32))
            | (1u64.wrapping_shl(VIRTIO_NET_F_STATUS as u32))
    }

    fn set_acked_features(&mut self, features: u64) -> Result<()> {
        if (features & !self.get_features()) != 0 {
            warn!("Guest tried to enable unsupported features: {:#x}", features);
        }
        Ok(())
    }

    fn read_config(&self, offset: u64, data: &mut [u8]) -> Result<()> {
        let config = NetConfig {
            mac: self.local_mac,
            status: 1,
            max_virtqueue_pairs: 1,
            mtu: 1500,
        };

        let config_slice = unsafe {
            std::slice::from_raw_parts(
                &config as *const _ as *const u8,
                std::mem::size_of::<NetConfig>(),
            )
        };

        let config_len = config_slice.len() as u64;
        if offset >= config_len {
            return Err(HypervisorError::MemoryError(
                "Invalid config space offset".into(),
            ));
        }

        let len = std::cmp::min(data.len() as u64, config_len - offset) as usize;
        data[..len].copy_from_slice(&config_slice[offset as usize..(offset as usize + len)]);

        Ok(())
    }

    fn write_config(&mut self, _offset: u64, _data: &[u8]) -> Result<()> {
        Ok(())
    }

    fn process_queue(&mut self, queue_idx: u32) -> Result<()> {
        match queue_idx {
            0 => self.process_rx_queue(),
            1 => self.process_tx_queue(),
            _ => Err(HypervisorError::MemoryError(
                format!("Invalid queue index: {}", queue_idx),
            )),
        }
    }

    fn get_queues(&self) -> Vec<u16> {
        self.config.queues().iter().map(|q| q.max_size()).collect()
    }

    fn get_interrupt_status(&self) -> u32 {
        0
    }
}
