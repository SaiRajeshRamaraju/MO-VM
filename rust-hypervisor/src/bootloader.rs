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
            
        Self::install_minimal_bios(memory)?;
            
        Ok(())
    }

    /// Set up a fake IVT and handlers for common BIOS interrupts.
    fn install_minimal_bios<M: GuestMemory>(memory: &M) -> Result<()> {
        let bios_addr = 0x1000u32;
        
        // --- INT 0x10 (Video Services) handler ---
        // For AH=0x0E (Teletype Output), AL contains the character.
        // Rather than parsing AH, we blindly write AL to COM1 (0x3F8)
        let int10_handler: [u8; 5] = [0xBA, 0xF8, 0x03, 0xEE, 0xCF]; // mov dx,0x3f8; out dx,al; iret
        memory.write_slice(&int10_handler, GuestAddress(bios_addr as u64))
            .map_err(|_| HypervisorError::MemoryError("Failed to write INT 0x10 handler".to_string()))?;
            
        // IVT entry for INT 0x10 (Vector 0x10 -> Address 0x40)
        let ivt_entry: [u8; 4] = [
            (bios_addr & 0xFF) as u8, ((bios_addr >> 8) & 0xFF) as u8, 
            0x00, 0x00
        ];
        memory.write_slice(&ivt_entry, GuestAddress(0x40))
            .map_err(|_| HypervisorError::MemoryError("Failed to write IVT entry 0x10".to_string()))?;
            
        // Dummy iret for other common generic interrupts
        let dummy_iret: [u8; 1] = [0xCF]; // iret
        memory.write_slice(&dummy_iret, GuestAddress((bios_addr as u64) + 0x10))
            .map_err(|_| HypervisorError::MemoryError("Failed to write dummy iret".to_string()))?;

        for i in &[0x13, 0x15, 0x16] { 
            let ivt_dummy: [u8; 4] = [
                ((bios_addr + 0x10) & 0xFF) as u8, (((bios_addr + 0x10) >> 8) & 0xFF) as u8, 
                0x00, 0x00
            ];
            memory.write_slice(&ivt_dummy, GuestAddress((*i as u64) * 4))
                .map_err(|_| HypervisorError::MemoryError("Failed to write dummy IVT entry".to_string()))?;
        }

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
        
        sregs.idt.base = 0;
        sregs.idt.limit = 0x3FF;
        sregs.gdt.base = 0;
        sregs.gdt.limit = 0xFFFF;

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
