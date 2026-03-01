use std::fs::File;
use std::io::Read;
use std::path::Path;
use log::info;
use vm_memory::{GuestAddress, GuestMemory, Bytes};
use crate::error::{HypervisorError, Result};
use crate::vcpu::Vcpu;

pub struct BiosLoader;

impl BiosLoader {
    pub fn load_bootloader<M: GuestMemory>(memory: &M, boot_path: &Path) -> Result<()> {
        info!("Loading bootloader from {:?}", boot_path);
        let mut file = File::open(boot_path).map_err(HypervisorError::IoError)?;
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).map_err(HypervisorError::IoError)?;

        if buffer.len() > 512 {
            log::warn!("Bootloader is larger than 512 bytes ({} bytes)", buffer.len());
        }

        let boot_addr = GuestAddress(0x7c00);
        memory.write_slice(&buffer, boot_addr)
            .map_err(|e| HypervisorError::MemoryError(format!("Failed to write bootloader: {}", e)))?;
            
        Ok(())
    }

    pub fn init_vcpu_for_real_mode(vcpu: &Vcpu) -> Result<()> {
        let mut sregs = vcpu.fd.get_sregs().map_err(HypervisorError::KvmError)?;
        
        // Reset to Real Mode defaults
        sregs.cs.selector = 0;
        sregs.cs.base = 0;
        sregs.cs.limit = 0xFFFF;
        sregs.cs.type_ = 11;
        sregs.cs.present = 1;
        sregs.cs.dpl = 0;
        sregs.cs.db = 0;
        sregs.cs.s = 1;
        sregs.cs.l = 0;
        sregs.cs.g = 0;
        sregs.cs.avl = 0;
        
        let mut data_seg = sregs.cs;
        data_seg.type_ = 3;
        
        sregs.ds = data_seg;
        sregs.es = data_seg;
        sregs.fs = data_seg;
        sregs.gs = data_seg;
        sregs.ss = data_seg;
        
        sregs.cr0 = 0x10; // Only ET bit set, PE is 0
        sregs.cr3 = 0;
        sregs.cr4 = 0;
        sregs.efer = 0;

        vcpu.fd.set_sregs(&sregs).map_err(HypervisorError::KvmError)?;
        
        let mut regs = vcpu.fd.get_regs().map_err(HypervisorError::KvmError)?;
        regs.rflags = 0x2;
        regs.rip = 0x7c00;
        regs.rsp = 0x7c00; // stack right before bootloader
        regs.rbp = 0;
        
        vcpu.fd.set_regs(&regs).map_err(HypervisorError::KvmError)?;
        
        Ok(())
    }
}
