use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::io;
use std::fs::{self, File, OpenOptions, Metadata};
use std::os::unix::fs::{FileTypeExt, PermissionsExt, MetadataExt};
use std::collections::HashMap;

use bitflags::bitflags;
use serde::{Serialize, Deserialize};
use log::{debug, error};
use vm_memory::Bytes;
use vhost_user_backend::{VhostUserBackendMut, VringRwLock, VringT};
use virtio_queue::QueueOwnedT;

use crate::error::{Result, HypervisorError};
use crate::virtio::VirtioDevice;

const MAX_IOVEC: usize = 128;
const MAX_IO_BYTES: usize = 64 * 1024; // 64KB max I/O size

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
        // Use the underlying u32 for serialization
        serializer.serialize_u32(self.bits())
    }
}

impl<'de> serde::Deserialize<'de> for AclPermissions {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // Deserialize as u32 and convert to AclPermissions
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
            owner: 0, // root
            group: 0, // root
            entries: Vec::new(),
        }
    }
}

impl Acl {
    pub fn check_permission(&self, uid: u32, gid: u32, required: AclPermissions) -> bool {
        // Check owner
        if self.owner == uid {
            return self.entries.iter()
                .find(|e| e.uid == uid)
                .map(|e| e.permissions.contains(required))
                .unwrap_or_else(|| {
                    // Default owner has all permissions
                    required.is_empty() || required == AclPermissions::all()
                });
        }

        // Check group
        if self.group == gid {
            return self.entries.iter()
                .filter(|e| e.gid == gid)
                .any(|e| e.permissions.contains(required));
        }

        // Check other
        self.entries.iter()
            .filter(|e| e.uid == uid || e.gid == gid)
            .any(|e| e.permissions.contains(required))
    }
}

#[derive(Debug)]
pub struct FsState {
    root: PathBuf,
    acls: RwLock<HashMap<PathBuf, Acl>>,
    mmio_region: Option<(u64, u64)>, // (base, size)
    mmio_data: RwLock<Vec<u8>>,
}

#[derive(Debug, Clone)]
pub struct FileAttr {
    pub ino: u64,
    pub size: u64,
    pub blocks: u64,
    pub atime: SystemTime,
    pub mtime: SystemTime,
    pub ctime: SystemTime,
    pub kind: FileType,
    pub perm: u16,
    pub nlink: u32,
    pub uid: u32,
    pub gid: u32,
    pub rdev: u32,
    pub blksize: u32,
}

impl FsState {
    pub fn new(root: &Path, mmio_region: Option<(u64, u64)>) -> Result<Self> {
        // Ensure the root directory exists
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
        
        // Initialize root ACL
        state.init_acl(root)?;
        
        Ok(state)
    }
    
    fn get_path(&self, fid: u64) -> Result<PathBuf> {
        // In a real implementation, we would maintain a mapping of FIDs to paths
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
    
    // MMIO methods
    pub fn mmio_read(&self, offset: u64, data: &mut [u8]) -> Result<()> {
        let mmio_data = self.mmio_data.read().unwrap();
        let offset = offset as usize;
        
        if offset + data.len() > mmio_data.len() {
            return Err(HypervisorError::IoError(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid offset"
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
                io::ErrorKind::InvalidInput,
                "Invalid offset"
            )));
        }
        
        mmio_data[offset..offset + data.len()].copy_from_slice(data);
        Ok(())
    }
    
    fn create_file(&self, path: &Path, _perm: u32) -> io::Result<File> {
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .open(path)?;
        if let Err(e) = self.init_acl(path) {
            error!("Failed to initialize ACL: {}", e);
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
    mem: GuestMemoryMmap,
    vrings: [Option<VringRwLock>; 2],
    queue_evts: [EventFd; 2],
    irq_trigger: EventFd,
    features: u64,
    acked_features: u64,
}

impl VirtioFs {
    pub fn new(root: &Path, mem: GuestMemoryMmap, irq_trigger: EventFd) -> Result<Self> {
        // Create the FsState without mmio_region for now
        let state = Arc::new(RwLock::new(FsState::new(root, None)?));
        
        Ok(Self {
            state,
            mem,
            vrings: [None, None],
            queue_evts,
            irq_trigger,
            features: 0,
            acked_features: 0,
        })
    }
    
    fn process_9p(&self, desc_chain: &[u8]) -> Result<()> {
        // Parse 9P message and dispatch to appropriate handler
        // This is a simplified version - a real implementation would parse the full 9P protocol
        
        if desc_chain.len() < 4 {
            return Err(HypervisorError::IoError(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid message"
            )));
        }
        
        let msg_type = desc_chain[4]; // 9P message type
        
        match msg_type {
            // Handle 9P message types
            // These are just stubs for now - implement as needed
            _msg_type => {
                debug!("Unhandled 9P message type: {}", msg_type);
                Err(HypervisorError::IoError(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "Operation not supported"
                )))
            }
        }
    }
    
    // 9P message handlers
    fn handle_version(&self, _data: &[u8]) -> Result<()> {
        // Return our version information
        // In a real implementation, we would parse the request and format a proper response
        Ok(())
    }
    
    fn handle_attach(&self, _data: &[u8]) -> Result<()> {
        // Handle attach request
        Ok(())
    }
    
    fn handle_walk(&self, _data: &[u8]) -> Result<()> {
        // Handle directory walk
        Ok(())
    }
    
    fn handle_getattr(&self, _data: &[u8]) -> Result<()> {
        // Handle get attribute request
        Ok(())
    }
    
    fn handle_open(&self, _data: &[u8]) -> Result<()> {
        // Handle file open
        Ok(())
    }
    
    fn handle_read(&self, _data: &[u8]) -> Result<()> {
        // Handle file read
        Ok(())
    }
    
    fn handle_write(&self, _data: &[u8]) -> Result<()> {
        // Handle file write
        Ok(())
    }
    
    fn handle_clunk(&self, _data: &[u8]) -> Result<()> {
        // Handle close/unlink
        Ok(())
    }
}

impl VirtioDevice for VirtioFs {
    type Error = crate::error::Error;
    fn device_type(&self) -> u32 {
        // Virtio device ID for filesystem is 26
        26
    }

    fn get_features(&self) -> u64 {
        // Basic features for Virtio FS
        (1 << 0) | (1 << 32)  // VIRTIO_F_VERSION_1 | VIRTIO_F_IOMMU_PLATFORM
    }

    fn get_config(&self, _offset: u32, _size: u32) -> Vec<u8> {
        // No configuration needed for VirtioFS
        Vec::new()
    }
    
    fn protocol_features(&self) -> vhost::vhost_user::message::VhostUserProtocolFeatures {
        vhost::vhost_user::message::VhostUserProtocolFeatures::empty()
    }
    
    fn set_event_idx(&mut self, _enable: bool) {
        // Not needed for our implementation
    }
    
    fn set_acked_features(&mut self, features: u64) -> Result<()> {
        self.acked_features = features;
        Ok(())
    }
    
    fn read_config(&self, _offset: u64, data: &mut [u8]) -> Result<()> {
        // No configuration to read for VirtioFS
        for (_i, byte) in data.iter_mut().enumerate() {
            *byte = 0;
        }
        Ok(())
    }
    
    fn write_config(&mut self, _offset: u64, _data: &[u8]) -> Result<()> {
        // No configuration to write for VirtioFS
        Ok(())
    }

    fn process_queue(&mut self, _queue_idx: u32) -> Result<()> {
        // Process the virtqueue
        // The actual queue processing is handled by VhostUserBackendMut
        Ok(())
    }

    fn get_queues(&self) -> Vec<u16> {
        // Return the number of queues (1 for Virtio FS)
        vec![256]  // Default queue size of 256
    }

    fn get_interrupt_status(&self) -> u32 {
        // No interrupts implemented yet
        0
    }
}

impl VhostUserBackendMut for VirtioFs {
    type Vring = VringRwLock;
    type Bitmap = ();
    
    fn num_queues(&self) -> usize {
        1
    }
    
    fn max_queue_size(&self) -> usize {
        256
    }
    
    fn features(&self) -> u64 {
        self.features
    }
    
    fn update_memory(&mut self, mem: vm_memory::atomic::GuestMemoryAtomic<vm_memory::GuestMemoryMmap>) -> Result<()> {
        self.mem = mem;
        Ok(())
    }
    
    fn protocol_features(&self) -> vhost::vhost_user::message::VhostUserProtocolFeatures {
        vhost::vhost_user::message::VhostUserProtocolFeatures::empty()
    }
    
    fn set_event_idx(&mut self, _enable: bool) {
        // Not needed for our implementation
    }
    
    fn handle_event(
        &mut self,
        device_event: u16,
        evset: EventSet,
        vrings: &[VringRwLock],
        _thread_id: usize,
    ) -> Result<()> {
        if device_event != 0 {
            return Err(HypervisorError::IoError(std::io::Error::new(
                std::io::ErrorKind::Other,
                "Unexpected device event",
            )));
        }
        
        if !evset.contains(EventSet::IN) {
            return Ok(());
        }
        
        // Process available requests
        if let Some(vring) = vrings.get(0) {
            let mut vring = vring.get_mut();
            
            // Get the queue mutably and process available descriptor chains
            let queue = vring.get_queue_mut();
            let mem = &self.mem;
            
            // Process available descriptor chains
            // This is a simplified version - in a real implementation, you would need to properly
            // handle the descriptor chain processing
            while let Some(desc_chain) = queue.iter(mem).next() {
                // Process the descriptor chain
                let len = desc_chain.len();
                let mut data = vec![0; len as usize];
                if let Err(e) = desc_chain.memory().read_slice(&mut data, desc_chain.addr()) {
                    error!("Failed to read descriptor chain data: {}", e);
                    continue;
                }
                
                if let Err(e) = self.process_9p(&data) {
                    error!("Error processing 9P request: {}", e);
                }
                
                // Complete the request
                vring.add_used(desc_chain.head_index(), len as u32);
                vring.signal_used_queue()?;
            }
        }
        
        // Signal interrupt if needed
        self.irq_trigger.write(1).map_err(|e| {
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
