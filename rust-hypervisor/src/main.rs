// =============================================================================
// main.rs — Entry point for the Rust Hypervisor
// =============================================================================
//
// This is the cli entry point for the hypervisor. It:
//   1. Parses command-line arguments (kernel path, vCPU count, devices, etc.)
//   2. Sets up logging with configurable verbosity
//   3. Creates a VirtualMachine instance
//   4. Attaches virtio devices (console, block, network, filesystem)
//   5. Optionally restores a previous VM snapshot
//   6. Loads the guest kernel (ELF format) into guest memory
//   7. Runs the VM (blocks until Ctrl+C or vCPU error)
//   8. Optionally saves a VM snapshot on exit
//
// =============================================================================

use std::path::{Path, PathBuf};
use std::process::exit;

use clap::Parser;                     // CLI argument parsing (derive macro)
use log::{error, info};               // Structured logging macros
use simplelog::{CombinedLogger, Config, LevelFilter, TermLogger, TerminalMode};

use rust_hypervisor::error::Result;   // Our custom Result<T, HypervisorError>
use rust_hypervisor::vm::VirtualMachine;

// Using clap for parser cause gpt told me it's the best .

#[derive(Parser, Debug)]
#[clap(version, about = "A work in progress KVM-based hypervisor in Rust")]
struct Args {
    /// Path to the Linux kernel (ELF format) to boot
    #[clap(short, long, default_value = "vmlinuz")]
    kernel: PathBuf,
    
    /// Number of vCPUs to create (each runs on its own OS thread)
    #[clap(short, long, default_value = "1")]
    cpus: u32,
    
    /// Amount of guest memory in MB (currently fixed at 128MB in memory.rs)
    /// NOTE:
    /// - Goal to boot tiny core in this.
    #[clap(short, long, default_value = "128")]
    memory: u64,
    
    /// Path to a raw disk image file for virtio-blk
    #[clap(short, long)]
    disk: Option<PathBuf>,
    
    /// If set, mount the disk image as read-only (guest cannot write)
    /// This is required for base image to maintain integrity with immutability.So if this is
    /// network shared iamge, a machine can't edit the file which lead to base image corruption.
    #[clap(long)]
    read_only: bool,
    
    /// Local UDP address for networking (e.g., "127.0.0.1:5000").
    /// Requires --peer to also be set.
    #[clap(long)]
    net: Option<String>,
    
    /// Peer UDP address for networking (where to send packets)
    #[clap(long)]
    peer: Option<String>,
    
    /// Share a host directory with the guest via virtio-fs (9P protocol).
    /// Can be specified multiple times: --share /path1 --share /path2
    #[clap(long)]
    share: Vec<String>,
    
    /// Save a JSON snapshot of VM state (vCPUs + memory) to this path on exit
    #[clap(long)]
    save_state: Option<PathBuf>,
    
    /// Restore VM state from a previously saved snapshot before running
    #[clap(long)]
    restore_state: Option<PathBuf>,
    
    /// Path to a 16-bit bootloader
    #[clap(long)]
    bootloader: Option<PathBuf>,
    
    /// Log verbosity level (error, warn, info, debug, trace)
    #[clap(long, default_value = "info")]
    log_level: String,
}

// =============================================================================
// main() — Application entry point
// =============================================================================

fn main() -> Result<()> {
    // --- Step 1: Parse CLI arguments ---
    let args = Args::parse();
    
    // --- Step 2: Initialize logging ---
    // Map the string log level to the simplelog LevelFilter enum.
    let level = match args.log_level.as_str() {
        "error" => LevelFilter::Error,
        "warn"  => LevelFilter::Warn,
        "debug" => LevelFilter::Debug,
        "trace" => LevelFilter::Trace,
        _       => LevelFilter::Info,   // default
    };
    
    // CombinedLogger lets us log to both terminal and (optionally) files.
    // Here we only use terminal output with color support.
    CombinedLogger::init(vec![
        TermLogger::new(
            level,
            Config::default(),
            TerminalMode::Mixed,        // Mixed = stdout for info, stderr for errors
            simplelog::ColorChoice::Auto,
        ),
    ]).unwrap();

    info!("=== Rust Hypervisor ===");
    info!("Kernel:  {:?}", args.kernel);
    info!("vCPUs:   {}", args.cpus);
    info!("Memory:  {}MB", args.memory);
    
    // --- Step 3: Create the Virtual Machine ---
    // This opens /dev/kvm, creates a VM fd, allocates guest memory,
    // registers it with KVM, and creates vCPU file descriptors.
    let mut vm = match VirtualMachine::new(args.cpus) {
        Ok(vm) => vm,
        Err(e) => {
            error!("Failed to create VM: {}", e);
            exit(1);
        }
    };
    
    // --- Step 4: Attach virtio devices ---
    
    // Console is always present (maps guest hvc0 to host stdout)
    vm.add_virtio_console()?;
    
    // Block device (virtual disk) — only if --disk was provided
    if let Some(disk_path) = args.disk {
        info!("Disk:    {:?} (read_only={})", disk_path, args.read_only);
        vm.add_virtio_block(disk_path, args.read_only)?;
    }
    
    // Network device — requires both --net and --peer
    if let Some(net_addr) = args.net {
        let peer = args.peer.unwrap_or_else(|| {
            error!("--peer is required when --net is specified");
            exit(1);
        });
        info!("Network: {} -> {}", net_addr, peer);
        vm.add_virtio_net(&net_addr, &peer)?;
    }
    
    // Shared directories — each --share becomes a virtio-fs device
    for share in &args.share {
        let host_path = Path::new(share);
        // Use the directory name as the mount tag (e.g., "shared" for /tmp/shared)
        let tag = host_path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("shared");
        
        info!("Share:   {:?} (tag={})", host_path, tag);
        vm.add_virtio_fs(tag, host_path, false)?;
    }
    
    // --- Step 5: Restore state if requested ---
    if let Some(restore_path) = &args.restore_state {
        info!("Restoring VM state from {:?}", restore_path);
        vm.restore_state(restore_path)?;
    }
    
    // --- Step 6: Load the kernel and/or bootloader ---
    if let Err(e) = vm.load_kernel(&args.kernel) {
        error!("Failed to load kernel: {}", e);
        exit(1);
    }
    
    if let Some(bootloader_path) = &args.bootloader {
        if let Err(e) = vm.load_bootloader(bootloader_path) {
            error!("Failed to load bootloader: {}", e);
            exit(1);
        }
    }
    
    // --- Step 7: Run the VM ---
    // This blocks until Ctrl+C is pressed or a vCPU error occurs.
    // Each vCPU gets its own thread; the main thread monitors for errors.
    info!("Starting VM execution...");
    if let Err(e) = vm.run() {
        error!("VM execution failed: {}", e);
    }
    
    // --- Step 8: Save state if requested ---
    if let Some(save_path) = &args.save_state {
        info!("Saving VM state to {:?}", save_path);
        vm.save_state(save_path)?;
    }
    
    info!("VM execution completed");
    Ok(())
}
