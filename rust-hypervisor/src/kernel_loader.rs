use std::fs::File;
use std::io::Read;
use std::path::Path;
use log::info;
use vm_memory::{Bytes, GuestAddress, GuestMemory};
use goblin::elf::Elf;
use crate::error::{HypervisorError, Result};

pub struct KernelLoader {
    entry_point: u64,
    cmdline: String,
}

impl KernelLoader {
    pub fn new() -> Self {
        Self {
            entry_point: 0,
            cmdline: String::new(),
        }
    }

    pub fn load_kernel<M: GuestMemory>(
        &mut self,
        memory: &M,
        kernel_path: &Path,
        cmdline: &str,
    ) -> Result<()> {
        info!("Loading kernel from {:?}", kernel_path);
        
        let mut file = File::open(kernel_path)
            .map_err(|e| HypervisorError::IoError(e))?;
        
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer)
            .map_err(|e| HypervisorError::IoError(e))?;
        
        let elf = Elf::parse(&buffer)
            .map_err(|_| HypervisorError::MemoryError("Invalid ELF format".to_string()))?;
        
        for phdr in &elf.program_headers {
            if phdr.p_type == goblin::elf::program_header::PT_LOAD {
                let mem_start = phdr.p_paddr as u64;
                let file_start = phdr.p_offset as usize;
                let file_end = (phdr.p_offset + phdr.p_filesz) as usize;
                
                if file_end > buffer.len() {
                    return Err(HypervisorError::MemoryError(
                        "Invalid ELF segment: extends past end of file".to_string(),
                    ));
                }
                
                memory
                    .write_slice(
                        &buffer[file_start..file_end],
                        GuestAddress(mem_start),
                    )
                    .map_err(|e| {
                        HypervisorError::MemoryError(format!(
                            "Failed to write kernel segment at 0x{:x}: {}",
                            mem_start, e
                        ))
                    })?;
                    
                info!(
                    "Loaded segment: guest addr 0x{:x} - 0x{:x} ({} bytes)",
                    mem_start,
                    mem_start + phdr.p_memsz,
                    phdr.p_filesz
                );
            }
        }
        
        self.entry_point = elf.entry as u64;
        self.cmdline = cmdline.to_string();
        
        // --- Virtual Memory (Page Tables for 4GB Identity Map) ---
        let page_table_base = 0x9000;
        let pml4_addr = GuestAddress(page_table_base);
        let pdpte_addr = GuestAddress(page_table_base + 0x1000);
        let pde_addr = GuestAddress(page_table_base + 0x2000);
        
        // Zero 6 pages (24 KB)
        memory.write_slice(&vec![0u8; 0x6000], pml4_addr)
            .map_err(|_| HypervisorError::MemoryError("Failed to zero page tables".to_string()))?;

        // 1. PML4 entry 0 points to PDPTE (Flags: Present | RW = 0x3)
        let pml4_entry = (pdpte_addr.0 | 0x3) as u64;
        let mut pml4_bytes = [0u8; 8];
        pml4_bytes.copy_from_slice(&pml4_entry.to_le_bytes());
        memory.write_slice(&pml4_bytes, pml4_addr).map_err(|_| HypervisorError::MemoryError("PML4 write failed".to_string()))?;

        // 2. PDPTE entries 0..3 point to 4 PDE pages
        for i in 0..4 {
            let pde_page = pde_addr.0 + (i as u64 * 0x1000);
            let pdpte_entry = (pde_page | 0x3) as u64;
            let mut pdpte_bytes = [0u8; 8];
            pdpte_bytes.copy_from_slice(&pdpte_entry.to_le_bytes());
            
            let addr = GuestAddress(pdpte_addr.0 + i as u64 * 8);
            memory.write_slice(&pdpte_bytes, addr).map_err(|_| HypervisorError::MemoryError("PDPTE write failed".to_string()))?;
        }

        // 3. PDEs: 4 pages * 512 entries = 2048 entries mapping 4GB using 2MB pages
        // Flags: Present | RW | Huge (0x80) -> 0x83
        for i in 0..2048 {
            let phys_addr = i as u64 * 0x200000;
            let pde_entry = (phys_addr | 0x83) as u64;
            let mut pde_bytes = [0u8; 8];
            pde_bytes.copy_from_slice(&pde_entry.to_le_bytes());
            
            let addr = GuestAddress(pde_addr.0 + i as u64 * 8);
            memory.write_slice(&pde_bytes, addr).map_err(|_| HypervisorError::MemoryError("PDE write failed".to_string()))?;
        }
        
        // --- Boot Params & Cmdline ---
        let cmdline_addr = 0x10000u64;
        let mut cmdline_cstr = cmdline.to_string();
        cmdline_cstr.push('\0');
        memory.write_slice(cmdline_cstr.as_bytes(), GuestAddress(cmdline_addr))
            .map_err(|_| HypervisorError::MemoryError("Failed to write cmdline".to_string()))?;
            
        let mut boot_params = vec![0u8; 4096];
        boot_params[0x210] = 0xFF; // type_of_loader
        let cmd_ptr = (cmdline_addr as u32).to_le_bytes();
        boot_params[0x228..0x22c].copy_from_slice(&cmd_ptr);
        
        // minimal e820 map: RAM from 0 to 4GB
        let e820_entries: u8 = 1;
        boot_params[0x1e8] = e820_entries;
        // Entry 0 at 0x2d0
        // addr (8), size (8), type (4)
        let map_addr = 0u64.to_le_bytes();
        boot_params[0x2d0..0x2d8].copy_from_slice(&map_addr);
        let map_size = 0x8000000u64.to_le_bytes(); // 128MB matching MEMORY_SIZE in memory.rs
        boot_params[0x2d8..0x2e0].copy_from_slice(&map_size);
        let map_type = 1u32.to_le_bytes(); // E820_RAM
        boot_params[0x2e0..0x2e4].copy_from_slice(&map_type);
        
        let boot_params_addr = 0x7000u64;
        memory.write_slice(&boot_params, GuestAddress(boot_params_addr))
            .map_err(|_| HypervisorError::MemoryError("Failed to write boot params".to_string()))?;
            
        info!("Kernel loaded. Entry point: 0x{:x}", self.entry_point);
        
        let mut first_bytes = vec![0u8; 16];
        memory.read_slice(&mut first_bytes, GuestAddress(self.entry_point)).unwrap();
        info!("First bytes at entry point: {:?}", first_bytes);
        
        Ok(())
    }
    
    pub fn get_entry_point(&self) -> u64 {
        self.entry_point
    }
    
    pub fn get_cmdline(&self) -> &str {
        &self.cmdline
    }
}
