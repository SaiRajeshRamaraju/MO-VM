// =============================================================================
// vcpu.rs — Virtual CPU Management
// =============================================================================
//
// Each Vcpu represents one virtual CPU inside the guest VM. Under the hood,
// a vCPU is a KVM file descriptor (VcpuFd) that the host uses to:
//   - Configure the CPU's initial register state (general purpose, segment, MSR)
//   - Run the guest code via KVM_RUN ioctl
//   - Handle VM exits (I/O port access, MMIO, etc.)
//
// LIFECYCLE:
//   1. VirtualMachine::new() creates VcpuFd via kvm.create_vcpu()
//   2. Vcpu::new() wraps the fd with an ID
//   3. Vcpu::init() sets up registers for protected mode (x86_64)
//   4. Vcpu::run() is called in a loop from a dedicated thread
//
// WHY DO WE NEED AN ID?
//   The ID identifies which vCPU this is (0, 1, 2...). KVM uses it internally
//   for APIC routing, and we use it for logging and snapshot matching.
// =============================================================================

use kvm_bindings::{kvm_msr_entry, Msrs};
use log::info;
use kvm_ioctls::{VcpuExit, VcpuFd};

use crate::error::{HypervisorError, Result};

/// Represents a single virtual CPU backed by a KVM vCPU file descriptor.
/// The `fd` field is the KVM vCPU fd used for all ioctl operations.
/// The `id` field uniquely identifies this vCPU within the VM.
pub struct Vcpu {
    /// KVM vCPU file descriptor — used for get/set_regs, run(), etc.
    pub fd: VcpuFd,
    /// Unique identifier for this vCPU (0-indexed).
    pub id: u64,
}

impl Vcpu {
    /// Wraps an existing KVM vCPU fd into our Vcpu struct.
    pub fn new(fd: VcpuFd, id: u64) -> Result<Self> {
        Ok(Self { fd, id })
    }

    /// Initializes the vCPU's register state for x86_64 protected mode.
    ///
    /// This sets up:
    /// - **General purpose registers**: RIP (instruction pointer), RSP/RBP (stack)
    /// - **Segment registers**: CS, DS, ES, FS, GS, SS for flat 32-bit protected mode
    /// - **FPU state**: x87 control word and MXCSR for SSE
    /// - **MSRs**: EFER (for SYSCALL support), STAR, LSTAR, CSTAR, SFMASK
    ///
    /// After this, the kernel loader will update RIP to the actual kernel entry point.
    pub fn init(&self) -> Result<()> {
        // ----- General Purpose Registers -----
        let mut regs = self.fd.get_regs().map_err(HypervisorError::KvmError)?;
        
        regs.rflags = 0x2;     // Bit 1 is always set (x86 requirement)
        regs.rip = 0x1000;     // Placeholder — kernel loader overwrites this
        regs.rsp = 0x80000;    // Stack pointer (grows downward from here)
        regs.rbp = 0x80000;    // Base pointer = stack pointer initially
        
        self.fd.set_regs(&regs).map_err(HypervisorError::KvmError)?;
        
        // ----- Segment Registers (for 32-bit protected mode) -----
        // We set up a flat memory model where all segments cover 0 to 4GB.
        let mut sregs = self.fd.get_sregs().map_err(HypervisorError::KvmError)?;
        
        // Code Segment (CS):
        sregs.cs.selector = 0x8;       // GDT entry 1 (Ring 0 code)
        sregs.cs.base = 0;             // Flat model — base at 0
        sregs.cs.limit = 0xFFFFF;      // 4GB limit with granularity=4KB
        sregs.cs.type_ = 0xB;          // Code, Execute/Read, Accessed
        sregs.cs.present = 1;          // Segment is present in memory
        sregs.cs.dpl = 0;              // Descriptor Privilege Level = Ring 0 (kernel)
        sregs.cs.db = 1;               // 32-bit default operand size
        sregs.cs.s = 1;                // This is a code/data segment (not system)
        sregs.cs.l = 0;                // Not 64-bit (would be 1 for long mode)
        sregs.cs.g = 1;                // Granularity: limit is in 4KB pages
        sregs.cs.avl = 0;              // Available for system software use
        
        // Data segments: same as CS for flat model (all point to same 4GB space)
        sregs.ds = sregs.cs;
        sregs.es = sregs.cs;
        sregs.fs = sregs.cs;
        sregs.gs = sregs.cs;
        sregs.ss = sregs.cs;
        
        self.fd.set_sregs(&sregs).map_err(HypervisorError::KvmError)?;
        
        // ----- Floating Point Unit (FPU) -----
        let mut fpu = self.fd.get_fpu().map_err(HypervisorError::KvmError)?;
        fpu.fcw = 0x37f;       // Default x87 control word (all exceptions masked)
        fpu.mxcsr = 0x1f80;    // Default MXCSR (SSE control/status register)
        self.fd.set_fpu(&fpu).map_err(HypervisorError::KvmError)?;
        
        // ----- Model-Specific Registers (MSRs) -----
        self.setup_msrs()?;
        
        Ok(())
    }

    /// Runs the vCPU until it exits, in simple we let the guest execute until VM Exis happens (I/O, MMIO, halt, etc.)
    ///
    /// The KVM_RUN ioctl transfers control to the guest. When the guest
    /// performs an operation that can't be handled in hardware (like an
    /// I/O port access), KVM returns a VcpuExit describing what happened.
    ///
    /// We handle:
    /// - `IoIn`: Guest reads from an I/O port (we return 0)
    /// - `IoOut`: Guest writes to an I/O port (we log it)
    /// - Everything else: returned as-is for the caller to handle
    pub fn run(&self) -> Result<VcpuExit<'_>> {
        match self.fd.run().map_err(HypervisorError::KvmError)? {
            VcpuExit::IoIn(addr, data) => {
                info!("I/O port read: 0x{:x}", addr);
                // For now, return 0 for all port reads.
                // In a real hypervisor, you'd dispatch to the appropriate device.
                if let Some(first_byte) = data.first_mut() {
                    *first_byte = 0;
                }
                Ok(VcpuExit::IoIn(addr, data))
            }
            VcpuExit::IoOut(addr, data) => {
                info!("I/O port write: 0x{:x} = {:?}", addr, data);
                // TODO: Route to virtio devices or serial console
                Ok(VcpuExit::IoOut(addr, data))
            }
            // Other exit reasons (Hlt, MmioRead, MmioWrite, Shutdown, etc.)
            exit_reason => Ok(exit_reason),
        }
    }
    
    /// Configures Model-Specific Registers (MSRs) for the vCPU.
    ///
    /// MSRs are x86 registers that control CPU features. We configure:
    /// - **EFER** (0xC0000080): Enable SYSCALL/SYSRET and Long Mode
    /// - **STAR** (0xC0000081): Segment selectors for SYSCALL/SYSRET
    /// - **LSTAR** (0xC0000082): 64-bit SYSCALL entry point
    /// - **CSTAR** (0xC0000083): Compatibility mode SYSCALL entry point
    /// - **SFMASK** (0xC0000084): Flags to mask during SYSCALL
    fn setup_msrs(&self) -> Result<()> {
        let msr_entries = [
            kvm_msr_entry {
                index: 0xc0000080,      // EFER MSR
                data: 0x501,            // SCE (bit 0) | LME (bit 8) | LMA (bit 10)
                ..Default::default()    // padding/reserved fields = 0
            },
            kvm_msr_entry {
                index: 0xc0000081,      // STAR — SYSCALL selector setup
                data: 0x0013000800000000, // CS/SS selectors for kernel and user
                ..Default::default()
            },
            kvm_msr_entry {
                index: 0xc0000082,      // LSTAR — SYSCALL entry point (64-bit)
                data: 0x1000,           // Placeholder — kernel will set this up
                ..Default::default()
            },
            kvm_msr_entry {
                index: 0xc0000083,      // CSTAR — SYSCALL entry (compat mode)
                data: 0x0,              // Not used
                ..Default::default()
            },
            kvm_msr_entry {
                index: 0xc0000084,      // SFMASK — flags to clear on SYSCALL
                data: 0x0,              // Don't mask any flags
                ..Default::default()
            }
        ];
        
        // Msrs is a FAM (Flexible Array Member) struct from kvm_bindings.
        // It wraps the array of kvm_msr_entry for the KVM_SET_MSRS ioctl.
        let msrs = Msrs::from_entries(&msr_entries)
            .map_err(|e| HypervisorError::GenericError(format!("MSR error: {:?}", e)))?;
        
        self.fd.set_msrs(&msrs).map_err(HypervisorError::KvmError)?;
        
        Ok(())
    }
}

impl Drop for Vcpu {
    fn drop(&mut self) {
        // VcpuFd's own Drop impl closes the KVM file descriptor.
        // We don't need to do anything extra here.
    }
}
