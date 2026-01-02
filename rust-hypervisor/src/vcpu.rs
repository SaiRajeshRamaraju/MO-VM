use kvm_bindings::kvm_msr_entry;
use log::info;
use kvm_ioctls::{VcpuExit, VcpuFd};

use crate::error::{HypervisorError, Result};

pub struct Vcpu {
    pub fd: VcpuFd,
    pub id: u64, // why do we need an id for virtual cpu ?
}
// Why ?
impl Vcpu {
    pub fn new(fd: VcpuFd, id: u64) -> Result<Self> {
        Ok(Self { fd, id })
    }

    pub fn init(&self) -> Result<()> {
        // Initialize VCPU registers
        let mut regs = self.fd.get_regs().map_err(HypervisorError::KvmError)?;
        
        // Set initial CPU state (x86_64 specific)
        regs.rflags = 0x2; // Clear all flags
        regs.rip = 0x1000; // Will be updated by kernel loader
        regs.rsp = 0x80000; // Stack pointer
        regs.rbp = 0x80000; // Base pointer
        
        self.fd.set_regs(&regs).map_err(HypervisorError::KvmError)?;
        
        // Set segment registers using kvm_sregs
        let mut sregs = self.fd.get_sregs().map_err(HypervisorError::KvmError)?;
        
        // Set code segment
        sregs.cs.selector = 0x8;
        sregs.cs.base = 0;
        sregs.cs.limit = 0xFFFFF;
        sregs.cs.type_ = 0xB;      // Code, Execute/Read, Accessed (0xB)
        sregs.cs.present = 1;
        sregs.cs.dpl = 0;          // Ring 0
        sregs.cs.db = 1;           // 32-bit segment
        sregs.cs.s = 1;            // Code/Data segment
        sregs.cs.l = 0;            // Not 64-bit code
        sregs.cs.g = 1;            // Granularity: 4KB
        sregs.cs.avl = 0;          // Available for use by system software
        
        // Set data segments
        sregs.ds = sregs.cs;
        sregs.es = sregs.cs;
        sregs.fs = sregs.cs;
        sregs.gs = sregs.cs;
        sregs.ss = sregs.cs;
        
        self.fd.set_sregs(&sregs).map_err(HypervisorError::KvmError)?;
        
        // Initialize FPU
        let mut fpu = self.fd.get_fpu().map_err(HypervisorError::KvmError)?;
        fpu.fcw = 0x37f; // Default x87 control word
        fpu.mxcsr = 0x1f80; // Default MXCSR
        self.fd.set_fpu(&fpu).map_err(HypervisorError::KvmError)?;
        
        // Setup MSRs
        self.setup_msrs()?;
        
        Ok(())
    }

    pub fn run(&self) -> Result<VcpuExit> {
        match self.fd.run().map_err(HypervisorError::KvmError)? {
            VcpuExit::IoIn(addr, data) => {
                info!("I/O port read: 0x{:x}", addr);
                // For now, just return 0 for all I/O reads
                if let Some(first_byte) = data.first_mut() {
                    *first_byte = 0;
                }
                Ok(VcpuExit::IoIn(addr, data))
            }
            VcpuExit::IoOut(addr, data) => {
                info!("I/O port write: 0x{:x} = {:?}", addr, data);
                // For now, just acknowledge the write
                Ok(VcpuExit::IoOut(addr, data))
            }
            exit_reason => Ok(exit_reason),
        }
    }
    
    fn setup_msrs(&self) -> Result<()> {
        // Create MSR entries for the MSRs we want to set
        let msr_entries = [
            // Enable SYSCALL/SYSRET instructions
            kvm_msr_entry {
                index: 0xc0000080, // EFER MSR
                data: 0x501,       // SCE | LME | LMA
                ..Default::default()
            },
            // Set up STAR, LSTAR, CSTAR, SFMASK MSRs for SYSCALL/SYSRET
            kvm_msr_entry {
                index: 0xc0000081, // STAR
                data: 0x0013000800000000,
                ..Default::default()
            },
            kvm_msr_entry {
                index: 0xc0000082, // LSTAR
                data: 0x1000,      // Will be updated by kernel loader
                ..Default::default()
            },
            kvm_msr_entry {
                index: 0xc0000083, // CSTAR
                data: 0x0,
                ..Default::default()
            },
            kvm_msr_entry {
                index: 0xc0000084, // SFMASK
                data: 0x0,
                ..Default::default()
            }
        ];
        
        // Create MSRs structure
        let msrs = Msrs::from_entries(&msr_entries)
            .map_err(|e| HypervisorError::FamError(e.to_string()))?;
        
        // Set the MSRs
        self.fd.set_msrs(&msrs).map_err(HypervisorError::KvmError)?;
        
        Ok(())
    }
}

impl Drop for Vcpu {
    fn drop(&mut self) {
        // The VcpuFd will be closed automatically when dropped
        // No need to manually close the file descriptor as it's managed by the VcpuFd Drop implementation
    }
}
