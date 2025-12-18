use std::fs::File;
use std::io::Read;
use std::path::Path;

use log::{info};
use vm_memory::{Bytes, GuestAddress, GuestMemory};

use crate::error::{HypervisorError, Result};

// Linux kernel header magic number
const LINUX_MAGIC: u32 = 0x5372_6446; // "\x7FELF" in little-endian

// Boot parameters structure (simplified)
#[repr(C, packed)]
struct BootParams {
    hdr: LinuxHeader,
    // Additional boot parameters would go here
}

#[repr(C, packed)]
struct LinuxHeader {
    magic: u32,            // Magic number
    hdr_size: u32,         // Header size
    kernel_version: u32,    // Kernel version
    load_flags: u32,       // Boot protocol flags
    kernel_alignment: u32,  // Physical addr alignment for kernel
    cmdline_size: u32,     // Maximum size of the command line
    cmd_line_ptr: u32,     // Pointer to command line
    initrd_addr_max: u32,  // Highest address for initrd
    kernel_alignment_offset: u32, // Physical addr alignment offset
    // ... more fields in the actual Linux header
}

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
        
        // Read kernel file
        let mut file = File::open(kernel_path)
            .map_err(|e| HypervisorError::IoError(e))?;
        
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer)
            .map_err(|e| HypervisorError::IoError(e))?;
        
        // Parse ELF
        let elf = Elf::parse(&buffer)
            .map_err(|_| HypervisorError::MemoryError("Invalid ELF format".to_string()))?;
        
        // Load each program header
        for phdr in &elf.program_headers {
            if phdr.p_type == goblin::elf::program_header::PT_LOAD {
                let mem_start = phdr.p_paddr as u64;
                let file_start = phdr.p_offset as usize;
                let file_end = (phdr.p_offset + phdr.p_filesz) as usize;
                
                if file_end > buffer.len() {
                    return Err(HypervisorError::MemoryError(
                        "Invalid ELF segment".to_string(),
                    ));
                }
                
                // Write the segment to guest memory
                memory
                    .write_slice(
                        &buffer[file_start..file_end],
                        GuestAddress(mem_start),
                    )
                    .map_err(|e| {
                        HypervisorError::MemoryError(format!(
                            "Failed to write kernel segment: {}",
                            e
                        ))
                    })?;
                    
                info!("Loaded segment: 0x{:x}-0x{:x}", mem_start, mem_start + phdr.p_memsz);
            }
        }
        
        // Save entry point
        self.entry_point = elf.entry as u64;
        self.cmdline = cmdline.to_string();
        
        info!("Kernel loaded at entry point: 0x{:x}", self.entry_point);
        
        Ok(())
    }
    
    pub fn get_entry_point(&self) -> u64 {
        self.entry_point
    }
    
    pub fn get_cmdline(&self) -> &str {
        &self.cmdline
    }
}
