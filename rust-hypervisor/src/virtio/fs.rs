use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::io;
use std::fs::{self, File, OpenOptions};
use std::os::unix::fs::MetadataExt;
use std::collections::HashMap;

use bitflags::bitflags;
use serde::{Serialize, Deserialize};
use log::debug;
use vm_memory::{Bytes, GuestAddressSpace};
use vmm_sys_util::eventfd::EventFd;
use vhost_user_backend::{VhostUserBackendMut, VringRwLock, VringT};
use virtio_queue::{QueueOwnedT, QueueT};

use crate::error::{Result, HypervisorError};
use crate::virtio::VirtioDevice;

const MAX_IOVEC: usize = 128;
const MAX_IO_BYTES: usize = 64 * 1024;

// ACL Permissions
bitflags! {
    #[derive(Default, Clone, Copy, PartialEq, Eq)]
    pub struct AclPermissions: u32 {
        const READ = 0b0001;
        const WRITE = 0b0010;
        const EXECUTE = 0b0100;
        const DELETE = 0b1000;
    }
}

impl serde::Serialize for AclPermissions {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_u32(self.bits())
    }
}

impl<'de> serde::Deserialize<'de> for AclPermissions {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let bits = u32::deserialize(deserializer).map_err(serde::de::Error::custom)?;
        AclPermissions::from_bits(bits).ok_or_else(|| serde::de::Error::custom("invalid AclPermissions value"))
    }
}

impl std::fmt::Debug for AclPermissions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut flags = Vec::new();
        if self.contains(AclPermissions::READ) { flags.push("READ"); }
        if self.contains(AclPermissions::WRITE) { flags.push("WRITE"); }
        if self.contains(AclPermissions::EXECUTE) { flags.push("EXECUTE"); }
        if self.contains(AclPermissions::DELETE) { flags.push("DELETE"); }
        write!(f, "AclPermissions({} | {})", self.bits(), flags.join(" | "))
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AclEntry {
    pub uid: u32,
    pub gid: u32,
    pub permissions: AclPermissions,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Acl {
    pub owner: u32,
    pub group: u32,
    pub entries: Vec<AclEntry>,
}

impl Default for Acl {
    fn default() -> Self {
        Self {
            owner: 0,
            group: 0,
            entries: Vec::new(),
        }
    }
}

impl Acl {
    pub fn check_permission(&self, uid: u32, gid: u32, required: AclPermissions) -> bool {
        if self.owner == uid {
            return self.entries.iter()
                .find(|e| e.uid == uid)
                .map(|e| e.permissions.contains(required))
                .unwrap_or_else(|| {
                    required.is_empty() || required == AclPermissions::all()
                });
        }
        if self.group == gid {
            return self.entries.iter()
                .filter(|e| e.gid == gid)
                .any(|e| e.permissions.contains(required));
        }
        self.entries.iter()
            .filter(|e| e.uid == uid || e.gid == gid)
            .any(|e| e.permissions.contains(required))
    }
}

#[derive(Debug)]
pub struct FsState {
    root: PathBuf,
    acls: RwLock<HashMap<PathBuf, Acl>>,
    mmio_region: Option<(u64, u64)>,
    mmio_data: RwLock<Vec<u8>>,
}

impl FsState {
    pub fn new(root: &Path, mmio_region: Option<(u64, u64)>) -> Result<Self> {
        if !root.exists() {
            fs::create_dir_all(root)?;
        }

        let mmio_data = if let Some((_, size)) = mmio_region {
            vec![0u8; size as usize]
        } else {
            Vec::new()
        };

        let state = Self {
            root: root.to_path_buf(),
            acls: RwLock::new(HashMap::new()),
            mmio_region,
            mmio_data: RwLock::new(mmio_data),
        };

        state.init_acl(root)?;
        Ok(state)
    }

    fn get_path(&self, fid: u64) -> Result<PathBuf> {
        Ok(self.root.join(fid.to_string()))
    }

    fn init_acl(&self, path: &Path) -> Result<()> {
        let metadata = fs::metadata(path)?;
        let mut acls = self.acls.write().unwrap();
        if !acls.contains_key(path) {
            let acl = Acl {
                owner: metadata.uid(),
                group: metadata.gid(),
                entries: Vec::new(),
            };
            acls.insert(path.to_path_buf(), acl);
        }
        Ok(())
    }

    pub fn set_acl(&self, path: &Path, acl: Acl) -> Result<()> {
        let mut acls = self.acls.write().unwrap();
        acls.insert(path.to_path_buf(), acl);
        Ok(())
    }

    pub fn get_acl(&self, path: &Path) -> Option<Acl> {
        let acls = self.acls.read().unwrap();
        acls.get(path).cloned()
    }

    pub fn mmio_read(&self, offset: u64, data: &mut [u8]) -> Result<()> {
        let mmio_data = self.mmio_data.read().unwrap();
        let offset = offset as usize;
        if offset + data.len() > mmio_data.len() {
            return Err(HypervisorError::IoError(io::Error::new(
                io::ErrorKind::InvalidInput, "Invalid offset"
            )));
        }
        data.copy_from_slice(&mmio_data[offset..offset + data.len()]);
        Ok(())
    }

    pub fn mmio_write(&self, offset: u64, data: &[u8]) -> Result<()> {
        let mut mmio_data = self.mmio_data.write().unwrap();
        let offset = offset as usize;
        if offset + data.len() > mmio_data.len() {
            return Err(HypervisorError::IoError(io::Error::new(
                io::ErrorKind::InvalidInput, "Invalid offset"
            )));
        }
        mmio_data[offset..offset + data.len()].copy_from_slice(data);
        Ok(())
    }

    fn create_file(&self, path: &Path, _perm: u32) -> io::Result<File> {
        let file = OpenOptions::new().create(true).write(true).open(path)?;
        if let Err(e) = self.init_acl(path) {
            return Err(io::Error::new(io::ErrorKind::Other, format!("Failed to initialize ACL: {}", e)));
        }
        Ok(file)
    }

    fn open_file(&self, path: &Path, read: bool, write: bool) -> io::Result<File> {
        let mut opts = OpenOptions::new();
        opts.read(read).write(write);
        opts.open(path)
    }
}

pub struct VirtioFs {
    state: Arc<RwLock<FsState>>,
    mem: vm_memory::atomic::GuestMemoryAtomic<vm_memory::mmap::GuestMemoryMmap>,
    vrings: [Option<VringRwLock>; 2],
    queue_evts: [EventFd; 2],
    irq_trigger: EventFd,
    features: u64,
    acked_features: u64,
}

impl VirtioFs {
    pub fn new(root: &Path, mem: vm_memory::atomic::GuestMemoryAtomic<vm_memory::mmap::GuestMemoryMmap>, irq_trigger: EventFd) -> Result<Self> {
        let state = Arc::new(RwLock::new(FsState::new(root, None)?));

        let evt0 = EventFd::new(0).map_err(|e| HypervisorError::IoError(e))?;
        let evt1 = EventFd::new(0).map_err(|e| HypervisorError::IoError(e))?;

        Ok(Self {
            state,
            mem,
            vrings: [None, None],
            queue_evts: [evt0, evt1],
            irq_trigger,
            features: 0,
            acked_features: 0,
        })
    }

    fn process_9p(&self, desc_chain: &[u8]) -> Result<()> {
        if desc_chain.len() < 4 {
            return Err(HypervisorError::IoError(io::Error::new(
                io::ErrorKind::InvalidInput, "Invalid message"
            )));
        }
        let msg_type = desc_chain[4];
        match msg_type {
            _msg_type => {
                debug!("Unhandled 9P message type: {}", msg_type);
                Err(HypervisorError::IoError(io::Error::new(
                    io::ErrorKind::Unsupported, "Operation not supported"
                )))
            }
        }
    }

    fn handle_version(&self, _data: &[u8]) -> Result<()> { Ok(()) }
    fn handle_attach(&self, _data: &[u8]) -> Result<()> { Ok(()) }
    fn handle_walk(&self, _data: &[u8]) -> Result<()> { Ok(()) }
    fn handle_getattr(&self, _data: &[u8]) -> Result<()> { Ok(()) }
    fn handle_open(&self, _data: &[u8]) -> Result<()> { Ok(()) }
    fn handle_read(&self, _data: &[u8]) -> Result<()> { Ok(()) }
    fn handle_write(&self, _data: &[u8]) -> Result<()> { Ok(()) }
    fn handle_clunk(&self, _data: &[u8]) -> Result<()> { Ok(()) }
}

impl VirtioDevice for VirtioFs {
    type Error = crate::error::HypervisorError;
    fn device_type(&self) -> u32 { 26 }

    fn get_features(&self) -> u64 {
        (1 << 0) | (1 << 32)
    }

    fn set_acked_features(&mut self, features: u64) -> Result<()> {
        self.acked_features = features;
        Ok(())
    }

    fn read_config(&self, _offset: u64, data: &mut [u8]) -> Result<()> {
        for byte in data.iter_mut() { *byte = 0; }
        Ok(())
    }

    fn write_config(&mut self, _offset: u64, _data: &[u8]) -> Result<()> { Ok(()) }

    fn process_queue(&mut self, _queue_idx: u32) -> Result<()> { Ok(()) }

    fn get_queues(&self) -> Vec<u16> { vec![256] }

    fn get_interrupt_status(&self) -> u32 { 0 }
}

impl VhostUserBackendMut for VirtioFs {
    type Vring = VringRwLock;
    type Bitmap = ();

    fn num_queues(&self) -> usize { 1 }
    fn max_queue_size(&self) -> usize { 256 }
    fn features(&self) -> u64 { self.features }

    fn update_memory(&mut self, mem: vm_memory::atomic::GuestMemoryAtomic<vm_memory::mmap::GuestMemoryMmap>) -> std::result::Result<(), std::io::Error> {
        self.mem = mem;
        Ok(())
    }

    fn protocol_features(&self) -> vhost::vhost_user::message::VhostUserProtocolFeatures {
        vhost::vhost_user::message::VhostUserProtocolFeatures::empty()
    }

    fn set_event_idx(&mut self, _enable: bool) {}

    fn handle_event(
        &mut self,
        device_event: u16,
        evset: vmm_sys_util::epoll::EventSet,
        vrings: &[VringRwLock],
        _thread_id: usize,
    ) -> std::result::Result<(), std::io::Error> {
        if device_event != 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "Unexpected device event",
            ));
        }

        if !evset.contains(vmm_sys_util::epoll::EventSet::IN) {
            return Ok(());
        }

        if let Some(vring) = vrings.get(0) {
            let mut vring = vring.get_mut();
            let queue = vring.get_queue_mut();
            let mem_guard = self.mem.memory();

            while let Some(mut desc_chain) = queue.iter(&*mem_guard).map_err(|e| {
                std::io::Error::new(std::io::ErrorKind::Other, format!("Queue iter error: {:?}", e))
            })?.next() {
                let head_index = desc_chain.head_index();
                let desc = match desc_chain.next() {
                    Some(d) => d,
                    None => continue,
                };
                let len = desc.len() as usize;
                let mut data = vec![0; len];
                if let Err(_) = mem_guard.read_slice(&mut data, desc.addr()) {
                    continue;
                }

                let _ = self.process_9p(&data);

                let _ = queue.add_used(&*mem_guard, head_index, len as u32);
            }
        }

        self.irq_trigger.write(1u64).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("Failed to signal irq: {}", e),
            )
        })?;

        Ok(())
    }
}

// Error type for the filesystem implementation
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Invalid message format")]
    InvalidMessage,

    #[error("Unsupported operation")]
    UnsupportedOperation,

    #[error("Permission denied")]
    PermissionDenied,

    #[error("Invalid offset")]
    InvalidOffset,

    #[error("EventFd error: {0}")]
    EventFd(std::io::Error),
}

impl From<Error> for std::io::Error {
    fn from(e: Error) -> Self {
        match e {
            Error::Io(e) => e,
            e => std::io::Error::new(std::io::ErrorKind::Other, e),
        }
    }
}
