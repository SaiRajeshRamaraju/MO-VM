use std::sync::{Arc, Mutex};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

use log::warn;
use vm_memory::{Address, Bytes, GuestAddress, GuestMemoryMmap, Le16, Le32, Le64};
use virtio_queue::{QueueOwnedT, QueueT};
use crate::virtio::VirtioDeviceConfig;

use crate::error::{HypervisorError, Result};
use super::VirtioDevice;

// Virtio block device ID
const VIRTIO_ID_BLOCK: u32 = 2;

// Feature bits
const VIRTIO_BLK_F_SIZE_MAX: u64 = 1 << 1;
const VIRTIO_BLK_F_SEG_MAX: u64 = 1 << 2;
const VIRTIO_BLK_F_GEOMETRY: u64 = 1 << 4;
const VIRTIO_BLK_F_RO: u64 = 1 << 5;
const VIRTIO_BLK_F_BLK_SIZE: u64 = 1 << 6;
const VIRTIO_BLK_F_FLUSH: u64 = 1 << 9;
const VIRTIO_BLK_F_TOPOLOGY: u64 = 1 << 10;
const VIRTIO_F_VERSION_1: u64 = 1 << 32;

// Request types
const VIRTIO_BLK_T_IN: u32 = 0;
const VIRTIO_BLK_T_OUT: u32 = 1;
const VIRTIO_BLK_T_FLUSH: u32 = 4;
const VIRTIO_BLK_T_GET_ID: u32 = 8;

// Request status
const VIRTIO_BLK_S_OK: u8 = 0;
const VIRTIO_BLK_S_IOERR: u8 = 1;
const VIRTIO_BLK_S_UNSUPP: u8 = 2;

// Block device configuration
#[repr(C, packed)]
struct BlockConfig {
    capacity: Le64,
    size_max: Le32,
    seg_max: Le32,
    geometry: BlockGeometry,
    block_size: Le32,
    topology: BlockTopology,
    writeback: u8,
    unused: [u8; 3],
    max_discard_sectors: Le32,
    max_discard_seg: u32,
    discard_sector_alignment: Le32,
    max_write_zeroes_sectors: Le32,
    max_write_zeroes_seg: u32,
    write_zeroes_may_unmap: u8,
    unused2: [u8; 3],
}

#[repr(C, packed)]
struct BlockGeometry {
    cylinders: Le16,
    heads: u8,
    sectors: u8,
}

#[repr(C, packed)]
struct BlockTopology {
    physical_block_exp: u8,
    alignment_offset: u8,
    min_io_size: Le16,
    opt_io_size: Le32,
}

pub struct VirtioBlock {
    config: VirtioDeviceConfig,
    disk: Arc<Mutex<File>>,
    disk_size: u64,
    block_size: u32,
    read_only: bool,
}

impl VirtioBlock {
    pub fn new(
        mem: GuestMemoryMmap,
        guest_mem: GuestAddress,
        irq: u32,
        disk_path: &Path,
        read_only: bool,
    ) -> Result<Self> {
        let file = if read_only {
            File::open(disk_path).map_err(|e| {
                HypervisorError::IoError(io::Error::new(
                    io::ErrorKind::Other,
                    format!("Failed to open disk image: {}", e),
                ))
            })?
        } else {
            OpenOptions::new()
                .read(true)
                .write(true)
                .open(disk_path)
                .map_err(|e| {
                    HypervisorError::IoError(io::Error::new(
                        io::ErrorKind::Other,
                        format!("Failed to open disk image: {}", e),
                    ))
                })?
        };

        let disk_size = file.metadata()?.len();
        let block_size = 512; // Standard block size

        let config = VirtioDeviceConfig::new(mem, guest_mem, irq, 1).map_err(|e| {
            HypervisorError::MemoryError(format!("Failed to create virtio config: {}", e))
        })?;

        Ok(Self {
            config,
            disk: Arc::new(Mutex::new(file)),
            disk_size,
            block_size,
            read_only,
        })
    }

    fn process_request(&self, desc_chain: virtio_queue::DescriptorChain<&GuestMemoryMmap>) -> Result<u8> {
        let mut disk = self.disk.lock().unwrap();
        let mem = self.config.memory();
        let mut status = VIRTIO_BLK_S_OK;

        for desc in desc_chain {
            let mut req_type_buf = [0u8; 4];
            mem.read_slice(&mut req_type_buf, desc.addr())
                .map_err(|_| HypervisorError::MemoryError("Failed to read request type".into()))?;
            let req_type = u32::from_le_bytes(req_type_buf);

            match req_type {
                VIRTIO_BLK_T_IN => self.handle_read(&mut disk, &desc, mem)?,
                VIRTIO_BLK_T_OUT => {
                    if self.read_only {
                        status = VIRTIO_BLK_S_IOERR;
                        break;
                    }
                    self.handle_write(&mut disk, &desc, mem)?;
                }
                VIRTIO_BLK_T_FLUSH => {
                    disk.flush().map_err(|e| {
                        HypervisorError::IoError(io::Error::new(
                            io::ErrorKind::Other,
                            format!("Failed to flush disk: {}", e),
                        ))
                    })?;
                }
                _ => {
                    status = VIRTIO_BLK_S_UNSUPP;
                    break;
                }
            }
        }

        Ok(status)
    }

    fn handle_read(
        &self,
        disk: &mut File,
        desc: &virtio_queue::Descriptor,
        mem: &GuestMemoryMmap,
    ) -> Result<()> {
        let mut sector_buf = [0u8; 8];
        mem.read_slice(&mut sector_buf, desc.addr())
            .map_err(|_| HypervisorError::MemoryError("Failed to read sector".into()))?;
        let sector = u64::from_le_bytes(sector_buf);

        let offset = sector * self.block_size as u64;
        disk.seek(SeekFrom::Start(offset))?;

        let buf_len = (desc.len() as usize).saturating_sub(16);
        let mut buf = vec![0u8; buf_len];
        disk.read_exact(&mut buf)?;

        let write_addr = desc.addr().checked_add(16).ok_or_else(|| {
            HypervisorError::MemoryError("Invalid descriptor address".into())
        })?;
        mem.write_slice(&buf, write_addr)?;

        Ok(())
    }

    fn handle_write(
        &self,
        disk: &mut File,
        desc: &virtio_queue::Descriptor,
        mem: &GuestMemoryMmap,
    ) -> Result<()> {
        let mut sector_buf = [0u8; 8];
        mem.read_slice(&mut sector_buf, desc.addr())
            .map_err(|_| HypervisorError::MemoryError("Failed to read sector".into()))?;
        let sector = u64::from_le_bytes(sector_buf);

        let offset = sector * self.block_size as u64;
        disk.seek(SeekFrom::Start(offset))?;

        let buf_len = (desc.len() as usize).saturating_sub(16);
        let mut buf = vec![0u8; buf_len];
        let read_addr = desc.addr().checked_add(16).ok_or_else(|| {
            HypervisorError::MemoryError("Invalid descriptor address".into())
        })?;
        mem.read_slice(&mut buf, read_addr)?;

        disk.write_all(&buf)?;
        Ok(())
    }
}

impl VirtioDevice for VirtioBlock {
    type Error = crate::error::HypervisorError;
    fn device_type(&self) -> u32 {
        VIRTIO_ID_BLOCK
    }

    fn get_features(&self) -> u64 {
        let mut features = VIRTIO_F_VERSION_1
            | VIRTIO_BLK_F_BLK_SIZE
            | VIRTIO_BLK_F_FLUSH
            | VIRTIO_BLK_F_TOPOLOGY;

        if self.read_only {
            features |= VIRTIO_BLK_F_RO;
        }

        features
    }

    fn set_acked_features(&mut self, features: u64) -> Result<()> {
        if (features & !self.get_features()) != 0 {
            warn!("Guest tried to enable unsupported features: {:#x}", features);
        }
        Ok(())
    }

    fn read_config(&self, offset: u64, data: &mut [u8]) -> Result<()> {
        let config = BlockConfig {
            capacity: Le64::from(self.disk_size / self.block_size as u64),
            size_max: Le32::from(131072),
            seg_max: Le32::from(32),
            geometry: BlockGeometry {
                cylinders: Le16::from(0),
                heads: 16,
                sectors: 63,
            },
            block_size: Le32::from(self.block_size),
            topology: BlockTopology {
                physical_block_exp: 0,
                alignment_offset: 0,
                min_io_size: Le16::from(512),
                opt_io_size: Le32::from(0),
            },
            writeback: 0,
            unused: [0; 3],
            max_discard_sectors: Le32::from(0),
            max_discard_seg: 0,
            discard_sector_alignment: Le32::from(0),
            max_write_zeroes_sectors: Le32::from(0),
            max_write_zeroes_seg: 0,
            write_zeroes_may_unmap: 0,
            unused2: [0; 3],
        };

        let config_slice = unsafe {
            std::slice::from_raw_parts(
                &config as *const _ as *const u8,
                std::mem::size_of::<BlockConfig>(),
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
        if queue_idx != 0 {
            return Err(HypervisorError::MemoryError(
                "Invalid queue index for block device".into(),
            ));
        }

        let mem = &self.config.mem;
        let disk = self.disk.clone();
        let block_size = self.block_size;
        let read_only = self.read_only;
        let queue = self.config.queues.get_mut(0).ok_or_else(|| {
            HypervisorError::MemoryError("Queue not found".into())
        })?;

        while let Some(desc_chain) = queue.iter(mem).map_err(|e| {
            HypervisorError::MemoryError(format!("Failed to iterate queue: {}", e))
        })?.next() {
            let head_index = desc_chain.head_index();
            
            // Inline process_request logic to avoid borrowing self
            let mut disk_guard = disk.lock().unwrap();
            let mut status = VIRTIO_BLK_S_OK;

            for desc in desc_chain {
                let mut req_type_buf = [0u8; 4];
                mem.read_slice(&mut req_type_buf, desc.addr())
                    .map_err(|_| HypervisorError::MemoryError("Failed to read request type".into()))?;
                let req_type = u32::from_le_bytes(req_type_buf);

                match req_type {
                    VIRTIO_BLK_T_IN => {
                        let mut sector_buf = [0u8; 8];
                        let _ = mem.read_slice(&mut sector_buf, desc.addr());
                        let sector = u64::from_le_bytes(sector_buf);
                        let offset = sector * block_size as u64;
                        let _ = disk_guard.seek(SeekFrom::Start(offset));
                        let buf_len = (desc.len() as usize).saturating_sub(16);
                        let mut buf = vec![0u8; buf_len];
                        let _ = disk_guard.read_exact(&mut buf);
                        if let Some(write_addr) = desc.addr().checked_add(16) {
                            let _ = mem.write_slice(&buf, write_addr);
                        }
                    }
                    VIRTIO_BLK_T_OUT => {
                        if read_only {
                            status = VIRTIO_BLK_S_IOERR;
                            break;
                        }
                        let mut sector_buf = [0u8; 8];
                        let _ = mem.read_slice(&mut sector_buf, desc.addr());
                        let sector = u64::from_le_bytes(sector_buf);
                        let offset = sector * block_size as u64;
                        let _ = disk_guard.seek(SeekFrom::Start(offset));
                        let buf_len = (desc.len() as usize).saturating_sub(16);
                        let mut buf = vec![0u8; buf_len];
                        if let Some(read_addr) = desc.addr().checked_add(16) {
                            let _ = mem.read_slice(&mut buf, read_addr);
                        }
                        let _ = disk_guard.write_all(&buf);
                    }
                    VIRTIO_BLK_T_FLUSH => { let _ = disk_guard.flush(); }
                    _ => { status = VIRTIO_BLK_S_UNSUPP; break; }
                }
            }
            drop(disk_guard);

            queue.add_used(mem, head_index, status as u32).map_err(|e| {
                HypervisorError::MemoryError(format!("Failed to add used descriptor: {}", e))
            })?;
        }

        if let Some(interrupt_evt) = &self.config.interrupt_evt {
            interrupt_evt.write(1u64).map_err(|e| {
                HypervisorError::IoError(io::Error::new(
                    io::ErrorKind::Other,
                    format!("Failed to signal interrupt: {}", e),
                ))
            })?;
        }

        Ok(())
    }

    fn get_queues(&self) -> Vec<u16> {
        self.config.queues().iter().map(|q| q.max_size()).collect()
    }

    fn get_interrupt_status(&self) -> u32 {
        0
    }
}
