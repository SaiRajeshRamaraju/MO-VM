use thiserror::Error;
use bincode;
use vm_memory::GuestMemoryError;

#[derive(Debug, thiserror::Error)]
pub enum HypervisorError {
    #[error("KVM error: {0}")]
    KvmError(#[from] kvm_ioctls::Error),
    
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
    
    #[error("Bincode error: {0}")]
    BincodeError(#[from] Box<bincode::ErrorKind>),
    
    #[error("Memory error: {0}")]
    MemoryError(String),
    
    #[error("Virtio error: {0:?}")]
    VirtioError(#[from] virtio_queue::Error),
    
    #[error("Unsupported architecture")]
    UnsupportedArchitecture,
    
    #[error("VCPU error: {0}")]
    VcpuError(String),
    
    #[error("Snapshot error: {0}")]
    SnapshotError(String),
    
    #[error("Thread spawn error: {0}")]
    ThreadSpawnError(String),
    
    #[error("Network error: {0}")]
    NetworkError(String),
    
    #[error("FAM error: {0}")]
    FamError(#[from] vmm_sys_util::fam::Error),
    
    #[error("Generic error: {0}")]
    GenericError(String),
}

impl From<&std::io::Error> for HypervisorError {
    fn from(err: &std::io::Error) -> Self {
        HypervisorError::IoError(std::io::Error::new(
            err.kind(),
            err.to_string()
        ))
    }
}

impl From<anyhow::Error> for HypervisorError {
    fn from(err: anyhow::Error) -> Self {
        if let Some(io_err) = err.downcast_ref::<std::io::Error>() {
            return HypervisorError::IoError(std::io::Error::new(
                io_err.kind(),
                io_err.to_string()
            ));
        }
        HypervisorError::GenericError(err.to_string())
    }
}

impl From<GuestMemoryError> for HypervisorError {
    fn from(err: GuestMemoryError) -> Self {
        HypervisorError::MemoryError(err.to_string())
    }
}

pub type Result<T> = std::result::Result<T, HypervisorError>;
