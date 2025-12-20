use std::path::{Path, PathBuf};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::time::Duration;
use std::thread;

use kvm_ioctls::{Kvm, VmFd};
use log::{debug, error, info};
use tokio::sync::{broadcast, mpsc, oneshot};
use vhost::vhost_user::message::VhostUserMemoryRegion;
use vm_memory::{GuestAddress, GuestMemory, Address};
use vmm_sys_util::eventfd::EventFd;

use crate::error::{HypervisorError, Result};
use crate::vcpu::Vcpu;
use crate::memory::{GuestMemoryRegions, setup_guest_memory};
use crate::virtio::VirtioDevice;
use crate::snapshot::SnapshotManager;

// Helper function to clone a VmFd
fn clone_vmfd(vm_fd: &VmFd) -> Result<VmFd> {
    // Use the KVM_CREATE_VM ioctl to create a new VM with the same parameters
    let kvm = Kvm::new().map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    let new_vm = kvm.create_vm().map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    
    // Copy the VM's attributes
    // Note: This is a simplified version - you might need to copy more attributes
    // depending on your specific use case
    if let Ok(attr) = vm_fd.get_clock() {
        if let Err(e) = new_vm.set_clock(&attr) {
            log::warn!("Failed to set KVM clock: {}", e);
        }
    }
    
    // Copy memory regions and other VM state would go here
    // This is non-trivial and depends on your specific requirements
    
    Ok(new_vm)
}


// Message type for communication between threads
enum VmMessage {
    Stop,
    SnapshotRequest(oneshot::Sender<Result<()>>, PathBuf),
}

pub struct VirtualMachine {
    kvm: Kvm,
    vm_fd: VmFd,
    vcpus: Vec<Vcpu>,
    memory: GuestMemoryRegions,
    kernel_loader: Option<KernelLoader>,
    devices: Vec<Box<dyn VirtioDevice<Error = crate::error::Error>>>,
    snapshot_manager: SnapshotManager,
    num_cpus: u32,
    next_irq: u32,
    running: Arc<AtomicBool>,
    message_tx: Option<mpsc::Sender<VmMessage>>,
}

impl VirtualMachine {
    pub fn new(num_cpus: u32) -> Result<Self> {
        // Initialize KVM
        let kvm = Kvm::new().map_err(HypervisorError::KvmError)?;
        
        // Create VM
        let vm_fd = kvm.create_vm().map_err(HypervisorError::KvmError)?;
        
        // Initialize guest memory
        let memory = GuestMemoryRegions::new()?;
        
        // Setup memory regions in KVM
        memory.setup_memory_region(&kvm, &vm_fd)?;
        
        // Create VCPUs
        let mut vcpus = Vec::with_capacity(num_cpus as usize);
        for i in 0..num_cpus {
            let vcpu_fd = vm_fd.create_vcpu(i as u64).map_err(HypervisorError::KvmError)?;
            let vcpu = Vcpu::new(vcpu_fd, i as u64)?;
            vcpus.push(vcpu);
        }
        
        // Create snapshot manager with the inner GuestMemoryMmap
        let snapshot_manager = SnapshotManager::new(
            Arc::new(memory.memory.clone()),
            clone_vmfd(&vm_fd)?
        );
        
        Ok(Self {
            kvm,
            vm_fd,
            vcpus,
            memory,
            kernel_loader: None,
            devices: Vec::new(),
            snapshot_manager,
            num_cpus,
            next_irq: 5, // Start IRQs after standard ones (0-4)
            running: Arc::new(AtomicBool::new(false)),
            message_tx: None,
        })
    }

    pub fn load_kernel(&mut self, kernel_path: &Path) -> Result<()> {
        let mut kernel_loader = KernelLoader::new();
        kernel_loader.load_kernel(
            &self.memory.memory, 
            kernel_path,
            "console=hvc0 root=/dev/vda rw" // Default kernel command line
        )?;
        self.kernel_loader = Some(kernel_loader);
        Ok(())
    }

    pub fn add_virtio_console(&mut self) -> Result<()> {
        use std::io::stdout;
        use std::sync::{Arc, Mutex};
        
        let irq = self.allocate_irq()?;
        let output = Arc::new(Mutex::new(stdout()));
        let console = VirtioConsole::new(
            self.memory.memory.clone(),
            GuestAddress(0x1000), // Example address
            irq,
            output
        )?;
        self.devices.push(Box::new(console));
        Ok(())
    }

    pub fn add_virtio_block(&mut self, disk_path: PathBuf, read_only: bool) -> Result<()> {
        let irq = self.allocate_irq()?;
        let block = VirtioBlock::new(
            self.memory.memory.clone(),
            GuestAddress(0x2000), // Example address
            irq,
            &disk_path,
            read_only
        )?;
        self.devices.push(Box::new(block));
        Ok(())
    }

    pub fn add_virtio_net(&mut self, local_addr: &str, peer_addr: &str) -> Result<()> {
        use std::net::SocketAddr;
        
        let irq = self.allocate_irq()?;
        let local_addr: SocketAddr = local_addr.parse().map_err(|e| {
            HypervisorError::NetworkError(format!("Invalid local address: {}", e))
        })?;
        let peer_addr: SocketAddr = peer_addr.parse().map_err(|e| {
            HypervisorError::NetworkError(format!("Invalid peer address: {}", e))
        })?;
        
        // Create a TAP interface for the network device
        let tap = crate::net::TunTap::new("tap0").map_err(|e| {
            HypervisorError::NetworkError(format!("Failed to create TAP interface: {}", e))
        })?;
        
        // For now, create a simple network device without TAP
        // In a real implementation, you would set up proper networking
        let net = VirtioNet::new(
            self.memory.memory.clone(),
            GuestAddress(0x3000), // Example address
            irq,
            local_addr.to_string().as_str(),
            None // No peer address for now
        )?;
        self.devices.push(Box::new(net));
        Ok(())
    }

    pub fn add_virtio_fs(&mut self, tag: &str, shared_dir: &Path, readonly: bool) -> Result<()> {
        use std::os::unix::io::FromRawFd;
        use std::fs::OpenOptions;
        use vhost_user_backend::VhostUserMemoryRegionInfo;
        use vhost::vhost_user::message::VhostUserMemoryRegion;
        
        let irq = self.allocate_irq()?;
        
        // Create an eventfd for IRQ triggering
        let event_fd = unsafe {
            let fd = libc::eventfd(0, libc::EFD_NONBLOCK | libc::EFD_CLOEXEC);
            if fd < 0 {
                return Err(HypervisorError::IoError(io::Error::last_os_error()));
            }
            std::os::unix::io::RawFd::from(fd)
        };
        
        let irq_trigger = unsafe { std::fs::File::from_raw_fd(event_fd) };
        
        // Convert memory regions to vhost-user format
        // Get memory regions using the GuestMemory trait
        use vm_memory::GuestMemoryRegion;
        let regions: Vec<VhostUserMemoryRegion> = self.memory.memory
            .iter()
            .map(|region| VhostUserMemoryRegion {
                guest_phys_addr: region.start_addr().raw_value(),
                memory_size: region.len() as u64,
                userspace_addr: 0, // Filled in by the vhost-user backend
                mmap_offset: 0,    // Filled in by the vhost-user backend
            })
            .collect();
        
        // Create a simple VirtioFS device
        // Note: In a real implementation, you would set up proper vhost-user backend
        
        // Create an EventFd for the queue
        let queue_evt = EventFd::new(0).map_err(|e| {
            HypervisorError::IoError(io::Error::new(
                io::ErrorKind::Other,
                format!("Failed to create EventFd: {}", e)
            ))
        })?;
        
        // Create the VirtioFS device
        let fs = crate::virtio::fs::VirtioFs::new(
            shared_dir,  // Pass the &Path directly
            self.memory.memory.clone(),  // Pass GuestMemoryMmap directly, not in an Arc
            queue_evt,
        )?;
        self.devices.push(Box::new(fs));
        Ok(())
    }

    fn allocate_irq(&mut self) -> Result<u32> {
        let irq = self.next_irq;
        self.next_irq += 1;
        Ok(irq)
    }

    pub fn run(&mut self) -> Result<()> {
        info!("Starting VM...");
        
        // Set running flag
        self.running.store(true, Ordering::SeqCst);
        
        // Create a channel for VCPU errors
        let (error_tx, error_rx) = mpsc::channel();
        let error_occurred = Arc::new(AtomicBool::new(false));
        
        // Create a channel for VM control messages
        let (message_tx, _) = broadcast::channel(32);  // Buffer size of 32
        self.message_tx = Some(message_tx);
        
        // Start all VCPUs in separate threads
        let mut handles = Vec::new();
        let running = self.running.clone();
        
        for (i, vcpu) in self.vcpus.iter().enumerate() {
            let vcpu = vcpu.clone();
            let running = running.clone();
            let error_tx = error_tx.clone();
            let error_occurred = error_occurred.clone();
            let message_rx = message_tx.subscribe(); // Now using tokio's broadcast
            
            let handle = thread::Builder::new()
                .name(format!("vcpu-{}", i))
                .spawn(move || {
                    let vcpu_id = i as u8;
                    info!("Starting VCPU {}", vcpu_id);
                    
                    while running.load(Ordering::SeqCst) && !error_occurred.load(Ordering::SeqCst) {
                        // Check for control messages
                        if let Ok(message) = message_rx.try_recv() {
                            match message {
                                VmMessage::Stop => {
                                    info!("VCPU {} received stop signal", vcpu_id);
                                    break;
                                }
                                VmMessage::SnapshotRequest(tx, _) => {
                                    // Only the first VCPU handles snapshot requests
                                    if vcpu_id == 0 {
                                        if let Err(e) = tx.send(Err(HypervisorError::SnapshotError("Snapshot not implemented".to_string()))) {
                                            error!("Failed to send snapshot response: {}", e);
                                        }
                                    }
                                }
                            }
                            continue;
                        }
                        
                        // Run the VCPU
                        match vcpu.run() {
                            Ok(exit_reason) => {
                                // Handle VM exit
                                debug!("VCPU {} exited with reason: {:?}", vcpu_id, exit_reason);
                                // In a real implementation, we would handle different exit reasons here
                                
                                // For now, just continue execution
                                continue;
                            }
                            Err(e) => {
                                error!("VCPU {} error: {}", vcpu_id, e);
                                if let Err(e) = error_tx.send(Some(e.to_string())) {
                                    error!("Failed to send error from VCPU {}: {}", vcpu_id, e);
                                }
                                error_occurred.store(true, Ordering::SeqCst);
                                break;
                            }
                        }
                    }
                    
                    info!("VCPU {} exiting", vcpu_id);
                })
                .map_err(|e| HypervisorError::ThreadSpawnError(e.to_string()))?;
            
            handles.push(handle);
        }
        
        // Main VM loop (for the main thread)
        let result = loop {
            // Check for errors from VCPU threads
            if let Ok(Some(err)) = error_rx.try_recv() {
                error!("Error from VCPU thread: {}", err);
                break Err(HypervisorError::VcpuError(format!("VCPU error: {}", err)));
            }
            
            // Check if any thread set the error flag
            if error_occurred.load(Ordering::SeqCst) {
                break Err(HypervisorError::VcpuError("VCPU thread encountered an error".to_string()));
            }
            
            // Check if we should stop
            if !self.running.load(Ordering::SeqCst) {
                break Ok(());
            }
            
            // Small sleep to prevent busy-waiting
            thread::sleep(Duration::from_millis(10));
        };
        
        // Stop all VCPUs
        self.stop()?;
        
        // Wait for all VCPU threads to finish
        for (i, handle) in handles.into_iter().enumerate() {
            if let Err(e) = handle.join() {
                error!("Failed to join VCPU thread {}: {:?}", i, e);
            }
        }
        
        info!("VM stopped");
        result
    }
    
    pub fn stop(&mut self) -> Result<()> {
        if self.running.load(Ordering::SeqCst) {
            info!("Stopping VM...");
            self.running.store(false, Ordering::SeqCst);
            
            // Send stop signal to all VCPUs
            if let Some(tx) = &self.message_tx {
                for _ in 0..self.num_cpus {
                    if let Err(e) = tx.send(VmMessage::Stop) {
                        error!("Failed to send stop signal to VCPU: {}", e);
                    }
                }
            }
        }
        
        // Create a thread for periodic snapshots
        let snapshot_thread = {
            let running = self.running.clone();
            let path_buf = path.to_path_buf();
            let message_tx = self.message_tx.as_ref().expect("VM not running").clone();
            let interval = Duration::from_secs(60); // Default to 60 seconds between snapshots
            
            thread::spawn(move || {
                while running.load(Ordering::SeqCst) {
                    thread::sleep(interval);
                    
                    // Create a channel to receive the snapshot result
                    let (tx, rx) = oneshot::channel();
                    
                    // Send snapshot request to the main thread
                    let snapshot_path = path.join(format!("snapshot_{}.bin", chrono::Local::now().format("%Y%m%d_%H%M%S")));
                    if let Err(e) = message_tx.send(VmMessage::SnapshotRequest(tx, snapshot_path)) {
                        error!("Failed to send snapshot request: {}", e);
                        continue;
                    }
                    
                    // Wait for the snapshot to complete
                    match rx.recv_timeout(Duration::from_secs(30)) {
                        Ok(Ok(())) => {
                            info!("Snapshot completed successfully");
                        }
                        Ok(Err(e)) => {
                            error!("Snapshot failed: {}", e);
                        }
                        Err(e) => {
                            error!("Failed to receive snapshot response: {}", e);
                        }
                    }
                }
                
                info!("Snapshot thread exiting");
            })
        };
        
        // Run the VM
        let result = self.run();
        
        // Stop the snapshot thread
        self.running.store(false, Ordering::SeqCst);
        if let Err(e) = snapshot_thread.join() {
            error!("Failed to join snapshot thread: {:?}", e);
        }
        
        result
    }
    
    pub fn save_state(&self, path: &Path) -> Result<()> {
        info!("Saving VM state to {:?}", path);
        self.snapshot_manager.save_to_file(path, &self.vcpus)
    }
    
    pub fn restore_state(&mut self, path: &Path) -> Result<()> {
        info!("Restoring VM state from {:?}", path);
        self.snapshot_manager.load_from_file(path, &mut self.vcpus)
    }
}

impl Drop for VirtualMachine {
    fn drop(&mut self) {
        // Stop the VM if it's running
        if self.running.load(Ordering::SeqCst) {
            if let Err(e) = self.stop() {
                error!("Failed to stop VM during drop: {}", e);
            }
        }
        
        // Clean up devices
        for device in &mut self.devices {
            if let Err(e) = device.shutdown() {
                error!("Failed to shutdown device: {}", e);
            }
        }
    }
}
