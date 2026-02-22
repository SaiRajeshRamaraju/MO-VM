// =============================================================================
// vm.rs — Virtual Machine Lifecycle Management
// =============================================================================
//
// This is the central orchestrator for the hypervisor. It ties together:
//   - KVM (the Linux kernel virtualization API)
//   - Guest memory
//   - vCPUs (virtual CPUs)
//   - Virtio devices (block, console, net, filesystem)
//   - Snapshot/restore
//
// LIFECYCLE:
//   VirtualMachine::new()       → Opens /dev/kvm, creates VM, allocates memory,
//                                 creates vCPUs, initializes snapshot manager
//   vm.add_virtio_*()           → Attaches virtio devices
//   vm.load_kernel()            → Loads an ELF kernel into guest memory
//   vm.run()                    → Starts vCPU threads, monitors for errors/Ctrl+C
//   vm.stop()                   → Signals all vCPU threads to exit
//   vm.save_state() / restore() → Snapshot/restore VM state
//   Drop                        → Stops VM and shuts down all devices
// =============================================================================

use std::io;
use crate::virtio::{VirtioConsole, VirtioBlock, VirtioNet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use kvm_ioctls::{Kvm, VmFd};
use log::{error, info};
use std::sync::mpsc::Sender;

use vm_memory::{GuestAddress, GuestMemory};
use vmm_sys_util::eventfd::EventFd;

use crate::error::{HypervisorError, Result};
use crate::vcpu::Vcpu;
use crate::memory::GuestMemoryRegions;
use crate::kernel_loader::KernelLoader;
use crate::virtio::VirtioDevice;
use crate::snapshot::SnapshotManager;

// =============================================================================
// Helper: Clone a VmFd
// =============================================================================
// KVM's VmFd doesn't implement Clone. To create a SnapshotManager that needs
// its own VmFd, we create a brand-new KVM VM and copy the clock state.
// This is a simplified approach — a production hypervisor would share the fd.

fn clone_vmfd(vm_fd: &VmFd) -> Result<VmFd> {
    let kvm = Kvm::new().map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    let new_vm = kvm.create_vm().map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    
    // Copy the KVM clock so snapshots have consistent timestamps
    if let Ok(attr) = vm_fd.get_clock() {
        if let Err(e) = new_vm.set_clock(&attr) {
            log::warn!("Failed to set KVM clock: {}", e);
        }
    }
    
    Ok(new_vm)
}

// =============================================================================
// Inter-thread Messages
// =============================================================================
// The main thread communicates with vCPU threads via message passing (channels).

/// Messages sent from the main thread to vCPU threads.
#[allow(dead_code)]
enum VmMessage {
    /// Tell the vCPU thread to exit its run loop.
    Stop,
    /// Request a VM snapshot. The vCPU thread sends the result back on the
    /// provided channel.
    SnapshotRequest(Sender<Result<()>>, PathBuf),
}

// =============================================================================
// VirtualMachine
// =============================================================================

/// The main hypervisor struct. Owns all VM resources.
pub struct VirtualMachine {
    // --- KVM handles ---
    /// KVM system handle (opened via /dev/kvm). Kept alive for the VM's lifetime.
    #[allow(dead_code)]
    kvm: Kvm,
    /// KVM VM file descriptor. Represents this specific VM instance in the kernel.
    #[allow(dead_code)]
    vm_fd: VmFd,
    
    // --- CPU and Memory ---
    /// All vCPUs belonging to this VM.
    vcpus: Vec<Vcpu>,
    /// Guest physical memory (mmap'd host memory registered with KVM).
    memory: GuestMemoryRegions,
    /// Kernel loader state (stores entry point after loading).
    kernel_loader: Option<KernelLoader>,
    
    // --- Devices ---
    /// All attached virtio devices (console, block, net, fs).
    /// Stored as trait objects since each device type is different.
    devices: Vec<Box<dyn VirtioDevice<Error = crate::error::HypervisorError>>>,
    
    // --- Snapshot ---
    /// Manages saving/restoring VM state (vCPU registers + memory).
    snapshot_manager: SnapshotManager,
    
    // --- Runtime state ---
    /// Number of vCPUs (used for thread setup).
    num_cpus: u32,
    /// Next available IRQ number. IRQs 0-4 are reserved for standard devices
    /// (timer, keyboard, cascade, COM2, COM1), so we start at 5.
    next_irq: u32,
    /// Shared flag: true = VM is running, false = stop all vCPU threads.
    /// Uses Arc<AtomicBool> so it can be shared across threads.
    running: Arc<AtomicBool>,
    /// Channels to send messages to each vCPU thread.
    message_tx: Vec<Sender<VmMessage>>,
}

impl VirtualMachine {
    /// Creates a new Virtual Machine with the specified number of vCPUs.
    ///
    /// This performs the full KVM setup sequence:
    /// 1. Open `/dev/kvm` → `Kvm` handle
    /// 2. Create a VM → `VmFd`
    /// 3. Allocate guest memory (mmap) and register it with KVM
    /// 4. Create vCPU file descriptors
    /// 5. Initialize the snapshot manager
    pub fn new(num_cpus: u32) -> Result<Self> {
        // Step 1: Open /dev/kvm — this gives us the KVM system fd
        let kvm = Kvm::new().map_err(HypervisorError::KvmError)?;
        
        // Step 2: Create a VM — this gives us an isolated address space in the kernel
        let vm_fd = kvm.create_vm().map_err(HypervisorError::KvmError)?;
        
        // Step 3: Allocate guest physical memory and register it with KVM
        // This creates an mmap'd region on the host and tells KVM to map
        // it into the guest's physical address space.
        let memory = GuestMemoryRegions::new()?;
        memory.setup_memory_region(&kvm, &vm_fd)?;
        
        // Step 4: Create vCPUs
        // Each vCPU gets its own KVM fd that can be used in a separate thread.
        let mut vcpus = Vec::with_capacity(num_cpus as usize);
        for i in 0..num_cpus {
            let vcpu_fd = vm_fd.create_vcpu(i as u64).map_err(HypervisorError::KvmError)?;
            let vcpu = Vcpu::new(vcpu_fd, i as u64)?;
            vcpus.push(vcpu);
        }
        
        // Step 5: Create snapshot manager (needs its own VmFd clone)
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
            next_irq: 5,   // IRQs 0-4 are reserved for legacy devices
            running: Arc::new(AtomicBool::new(false)),
            message_tx: Vec::new(),
        })
    }

    /// Loads a Linux kernel (ELF format) into guest memory.
    ///
    /// The kernel command line "console=hvc0 root=/dev/vda rw" tells the guest
    /// kernel to use the virtio console (hvc0) and mount /dev/vda as root.
    pub fn load_kernel(&mut self, kernel_path: &Path) -> Result<()> {
        let mut kernel_loader = KernelLoader::new();
        kernel_loader.load_kernel(
            &self.memory.memory, 
            kernel_path,
            "console=hvc0 root=/dev/vda rw"
        )?;
        self.kernel_loader = Some(kernel_loader);
        Ok(())
    }

    // =========================================================================
    // Device Attachment Methods
    // =========================================================================
    // Each add_virtio_* method:
    //   1. Allocates an IRQ number
    //   2. Creates the device with the shared guest memory
    //   3. Pushes it into the `devices` Vec as a trait object

    /// Attaches a virtio console (output connected to host stdout).
    ///
    /// The guest sees this as /dev/hvc0 and can write to it.
    /// Console input is not currently implemented (would need stdin handling).
    pub fn add_virtio_console(&mut self) -> Result<()> {
        use std::io::stdout;
        use std::sync::Mutex;
        
        let irq = self.allocate_irq()?;
        let output = Arc::new(Mutex::new(stdout()));
        let console = VirtioConsole::new(
            self.memory.memory.clone(),
            GuestAddress(0x1000),       // MMIO base address in guest physical memory
            irq,
            output
        )?;
        self.devices.push(Box::new(console));
        Ok(())
    }

    /// Attaches a virtio block device backed by a disk image file.
    ///
    /// The guest sees this as /dev/vda. If `read_only` is true, writes will
    /// return I/O errors to the guest.
    pub fn add_virtio_block(&mut self, disk_path: PathBuf, read_only: bool) -> Result<()> {
        let irq = self.allocate_irq()?;
        let block = VirtioBlock::new(
            self.memory.memory.clone(),
            GuestAddress(0x2000),
            irq,
            &disk_path,
            read_only
        )?;
        self.devices.push(Box::new(block));
        Ok(())
    }

    /// Attaches a virtio network device using UDP sockets for transport.
    ///
    /// This is a simple userspace networking approach (no TAP device needed).
    /// Packets are sent/received via UDP between the hypervisor and a peer.
    pub fn add_virtio_net(&mut self, local_addr: &str, peer_addr: &str) -> Result<()> {
        use std::net::SocketAddr;
        
        let irq = self.allocate_irq()?;
        
        // Validate addresses can be parsed
        let _local: SocketAddr = local_addr.parse().map_err(|e| {
            HypervisorError::NetworkError(format!("Invalid local address: {}", e))
        })?;
        let _peer: SocketAddr = peer_addr.parse().map_err(|e| {
            HypervisorError::NetworkError(format!("Invalid peer address: {}", e))
        })?;
        
        let net = VirtioNet::new(
            self.memory.memory.clone(),
            GuestAddress(0x3000),
            irq,
            local_addr,
            None    // TODO: pass peer_addr for actual networking
        )?;
        self.devices.push(Box::new(net));
        Ok(())
    }

    /// Attaches a virtio-fs device sharing a host directory.
    ///
    /// This creates a VirtioFs device that implements the 9P protocol
    /// for sharing files between host and guest.
    pub fn add_virtio_fs(&mut self, _tag: &str, shared_dir: &Path, _readonly: bool) -> Result<()> {
        let _irq = self.allocate_irq()?;
        
        // Create an EventFd for queue notifications
        let queue_evt = EventFd::new(0).map_err(|e| {
            HypervisorError::IoError(io::Error::new(
                io::ErrorKind::Other,
                format!("Failed to create EventFd: {}", e)
            ))
        })?;
        
        // GuestMemoryAtomic wraps memory with atomic reference counting
        // for thread-safe access from the vhost-user backend.
        let fs = crate::virtio::fs::VirtioFs::new(
            shared_dir,
            vm_memory::atomic::GuestMemoryAtomic::new(self.memory.memory.clone()),
            queue_evt,
        )?;
        self.devices.push(Box::new(fs));
        Ok(())
    }

    /// Allocates the next available IRQ number.
    /// IRQ numbers are sequential starting from 5.
    fn allocate_irq(&mut self) -> Result<u32> {
        let irq = self.next_irq;
        self.next_irq += 1;
        Ok(irq)
    }

    // =========================================================================
    // VM Execution
    // =========================================================================

    /// Runs the VM until stopped (Ctrl+C or vCPU error).
    ///
    /// # Threading Model
    /// - Each vCPU gets its own OS thread (via `std::thread::scope`)
    /// - The main thread monitors for errors and handles stop signals
    /// - vCPU threads communicate with the main thread via channels
    ///
    /// # Scoped Threads
    /// We use `std::thread::scope` (stable since Rust 1.63) so that vCPU
    /// threads can borrow `self.vcpus` without needing Arc or 'static lifetimes.
    /// All threads are guaranteed to exit before `scope` returns.
    pub fn run(&mut self) -> Result<()> {
        info!("Starting VM...");
        
        // Mark VM as running (shared across all threads)
        self.running.store(true, Ordering::SeqCst);
        
        // Create an error reporting channel
        // vCPU threads send errors here; main thread receives them
        let (error_tx, error_rx) = std::sync::mpsc::channel();
        let error_occurred = std::sync::atomic::AtomicBool::new(false);
        
        // Create per-vCPU message channels for stop/snapshot commands
        self.message_tx.clear();
        let mut vcpu_rxs = Vec::new();
        for _ in 0..self.num_cpus {
            let (tx, rx) = std::sync::mpsc::channel();
            self.message_tx.push(tx);
            vcpu_rxs.push(rx);
        }
        
        // --- Spawn vCPU threads in a scoped context ---
        std::thread::scope(|s| {
            // Spawn one thread per vCPU
            for (i, vcpu) in self.vcpus.iter().enumerate() {
                let error_tx = error_tx.clone();
                let error_occurred = &error_occurred;
                let running = &self.running;
                let message_rx = vcpu_rxs.remove(0); // Each thread gets its own receiver
                
                s.spawn(move || {
                    let vcpu_id = i as u8;
                    info!("Starting VCPU {}", vcpu_id);
                    
                    // vCPU run loop — keeps executing guest code until stopped
                    while running.load(Ordering::SeqCst) && !error_occurred.load(Ordering::SeqCst) {
                        // Check for control messages (non-blocking)
                        if let Ok(message) = message_rx.try_recv() {
                            match message {
                                VmMessage::Stop => break,
                                VmMessage::SnapshotRequest(tx, _) => {
                                    // Only vCPU 0 handles snapshot requests
                                    if vcpu_id == 0 {
                                        let _ = tx.send(Err(HypervisorError::SnapshotError(
                                            "Snapshot not implemented".to_string()
                                        )));
                                    }
                                }
                            }
                            continue;
                        }
                        
                        // Execute one guest instruction batch via KVM_RUN
                        match vcpu.run() {
                            Ok(_) => continue,
                            Err(e) => {
                                // Report error and signal other threads to stop
                                let _ = error_tx.send(Some(e.to_string()));
                                error_occurred.store(true, Ordering::SeqCst);
                                break;
                            }
                        }
                    }
                });
            }
            
            // --- Main thread: monitor loop ---
            // Polls for vCPU errors and checks if the running flag was cleared
            // (e.g., by Ctrl+C handler or vm.stop())
            loop {
                // Check if any vCPU thread reported an error
                if let Ok(Some(_err)) = error_rx.try_recv() {
                    self.running.store(false, Ordering::SeqCst);
                    break;
                }
                
                if error_occurred.load(Ordering::SeqCst) {
                    self.running.store(false, Ordering::SeqCst);
                    break;
                }
                
                // Check if we were stopped (e.g., by Ctrl+C)
                if !self.running.load(Ordering::SeqCst) {
                    break;
                }
                
                // Sleep briefly to avoid busy-waiting
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        });
        // At this point, all scoped threads have joined (guaranteed by thread::scope)
        
        self.stop().ok();
        
        info!("VM stopped");
        Ok(())
    }

    /// Stops the VM by signaling all vCPU threads to exit.
    pub fn stop(&mut self) -> Result<()> {
        if self.running.load(Ordering::SeqCst) {
            info!("Stopping VM...");
            self.running.store(false, Ordering::SeqCst);
            
            // Send Stop message to each vCPU thread
            for tx in &self.message_tx {
                let _ = tx.send(VmMessage::Stop);
            }
        }
        Ok(())
    }

    // =========================================================================
    // Snapshot / Restore
    // =========================================================================

    /// Saves the entire VM state (vCPU registers + guest memory) to a JSON file.
    pub fn save_state(&self, path: &Path) -> Result<()> {
        info!("Saving VM state to {:?}", path);
        self.snapshot_manager
            .save_to_file(path, &self.vcpus)
            .map_err(|e| HypervisorError::IoError(
                std::io::Error::new(std::io::ErrorKind::Other, e.to_string())
            ))
    }
    
    /// Restores VM state from a previously saved JSON snapshot.
    pub fn restore_state(&mut self, path: &Path) -> Result<()> {
        info!("Restoring VM state from {:?}", path);
        self.snapshot_manager
            .load_from_file(path, &mut self.vcpus)
            .map_err(|e| HypervisorError::IoError(
                std::io::Error::new(std::io::ErrorKind::Other, e.to_string())
            ))
    }
}

// =============================================================================
// Cleanup
// =============================================================================

impl Drop for VirtualMachine {
    fn drop(&mut self) {
        // Ensure VM is stopped before freeing resources
        if self.running.load(Ordering::SeqCst) {
            if let Err(e) = self.stop() {
                error!("Failed to stop VM during drop: {}", e);
            }
        }
        
        // Gracefully shut down all virtio devices
        for device in &mut self.devices {
            if let Err(e) = device.shutdown() {
                error!("Failed to shutdown device: {}", e);
            }
        }
    }
}
