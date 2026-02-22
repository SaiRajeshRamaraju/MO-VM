// =============================================================================
// kernel_loader.rs — ELF Kernel Loader
// =============================================================================
//
// This module loads a Linux kernel in ELF format into guest physical memory.
//
// HOW IT WORKS:
//   1. Read the kernel file from disk into a buffer
//   2. Parse the ELF headers using the `goblin` crate
//   3. For each PT_LOAD segment (i.e., segments that should be loaded into RAM):
//      - Calculate the guest physical address from the ELF program header (p_paddr)
//      - Copy the segment data from the file into guest memory at that address
//   4. Record the ELF entry point (where execution begins)
//
// The entry point is then used by vm.rs to set vCPU 0's RIP register.
// NOTE: This is a simplified loader. A hypervisor would also:
//   - Set up boot parameters (struct boot_params / zero page)
//   - Write the kernel command line to a known guest address
//   - Set up an initial page table for long mode
//   - Handle bzImage format (compressed kernels), not just raw ELF
// WARNING:
//  - This is future me problem to implement these
// =============================================================================

use std::fs::File;
use std::io::Read;
use std::path::Path;
use log::info;
use vm_memory::{Bytes, GuestAddress, GuestMemory};
use goblin::elf::Elf;
use crate::error::{HypervisorError, Result};

// Linux kernel header magic. Not currently used for bzImage detection,
// but kept here for reference if bzImage support is added later.
const LINUX_MAGIC: u32 = 0x5372_6446;

// Simplified Linux boot header structure.
// In a real bootloader, this would be the zero page (boot_params) at address 0x7000.
// Currently unused — kept as documentation of the Linux boot protocol.
#[repr(C, packed)]
struct BootParams {
    hdr: LinuxHeader,
    // The full boot_params struct has ~200 more fields (e820 map, etc.)
}

#[repr(C, packed)]
struct LinuxHeader {
    magic: u32,                         // Should be 0x53726446 ("HdrS")
    hdr_size: u32,                      // Size of this header
    kernel_version: u32,                // Kernel version number
    load_flags: u32,                    // Boot protocol flags
    kernel_alignment: u32,              // Required physical alignment
    cmdline_size: u32,                  // Max command line length
    cmd_line_ptr: u32,                  // Physical address of command line string
    initrd_addr_max: u32,               // Max address for initial ramdisk
    kernel_alignment_offset: u32,       // Alignment offset for relocation
    // ... many more fields in the real header
}

/// Loads a Linux kernel (ELF format) into guest memory.
///
/// After loading, call `get_entry_point()` to get the address where
/// the guest should start executing.
pub struct KernelLoader {
    /// The ELF entry point address (set after load_kernel succeeds)
    entry_point: u64,
    /// The kernel command line string (passed to the kernel via boot params)
    cmdline: String,
}

impl KernelLoader {
    /// Creates a new, uninitialized kernel loader.
    pub fn new() -> Self {
        Self {
            entry_point: 0,
            cmdline: String::new(),
        }
    }

    /// Loads an ELF kernel from `kernel_path` into `memory`.
    ///
    /// # Arguments
    /// - `memory`: Guest physical memory to write segments into
    /// - `kernel_path`: Path to the kernel ELF file on the host
    /// - `cmdline`: Kernel command line (e.g., "console=hvc0 root=/dev/vda rw")
    ///
    /// # How ELF Loading Works
    /// An ELF file contains "program headers" that describe segments:
    /// - `PT_LOAD` segments are the code/data that should be loaded into RAM
    /// - `p_paddr` is the physical address where the segment should go
    /// - `p_offset` / `p_filesz` is the offset and size within the ELF file
    /// - `p_memsz` may be larger than `p_filesz` (BSS segment — zero-filled)
    ///
    /// We copy each PT_LOAD segment from the file into guest memory.
    pub fn load_kernel<M: GuestMemory>(
        &mut self,
        memory: &M,
        kernel_path: &Path,
        cmdline: &str,
    ) -> Result<()> {
        info!("Loading kernel from {:?}", kernel_path);
        
        // Read the entire kernel file into memory
        let mut file = File::open(kernel_path)
            .map_err(|e| HypervisorError::IoError(e))?;
        
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer)
            .map_err(|e| HypervisorError::IoError(e))?;
        
        // Parse the ELF headers
        let elf = Elf::parse(&buffer)
            .map_err(|_| HypervisorError::MemoryError("Invalid ELF format".to_string()))?;
        
        // Load each PT_LOAD segment into guest memory
        for phdr in &elf.program_headers {
            // Only process loadable segments
            if phdr.p_type == goblin::elf::program_header::PT_LOAD {
                let mem_start = phdr.p_paddr as u64;       // Guest physical address
                let file_start = phdr.p_offset as usize;   // Offset in ELF file
                let file_end = (phdr.p_offset + phdr.p_filesz) as usize;
                
                // Sanity check: segment data must not extend past end of file
                if file_end > buffer.len() {
                    return Err(HypervisorError::MemoryError(
                        "Invalid ELF segment: extends past end of file".to_string(),
                    ));
                }
                
                // Write the segment bytes from the ELF file into guest RAM
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
        
        // Save the entry point — this is where the CPU should jump to
        self.entry_point = elf.entry as u64;
        self.cmdline = cmdline.to_string();
        
        info!("Kernel loaded. Entry point: 0x{:x}", self.entry_point);
        
        Ok(())
    }
    
    /// Returns the kernel entry point (the address where RIP should be set).
    pub fn get_entry_point(&self) -> u64 {
        self.entry_point
    }
    
    /// Returns the kernel command line string.
    pub fn get_cmdline(&self) -> &str {
        &self.cmdline
    }
}
