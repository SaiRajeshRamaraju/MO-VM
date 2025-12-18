mod error;
mod memory;
mod vcpu;
mod vm;

use std::path::PathBuf;
use std::process::exit;

use clap::Parser;
use log::{error, info};
use simplelog::{CombinedLogger, Config, LevelFilter, TermLogger, TerminalMode};

use crate::error::Result;
use crate::vm::VirtualMachine;

#[derive(Parser, Debug)]
#[clap(version, about = "A minimal hypervisor in Rust")]
struct Args {
    /// Path to the Linux kernel to boot
    #[clap(short, long, default_value = "vmlinuz")]
    kernel: PathBuf,
    
    /// Number of vCPUs to create
    #[clap(short, long, default_value = "1")]
    cpus: u32,
    
    /// Amount of memory in MB
    #[clap(short, long, default_value = "128")]
    memory: u64,
    
    /// Enable networking with the specified local address (e.g., "127.0.0.1:0")
    #[clap(long)]
    net: Option<String>,
    
    /// Peer address for networking (required if --net is specified)
    #[clap(long)]
    peer: Option<String>,
    
    /// Share a host directory with the guest
    /// Format: /host/path[:/guest/mount/path][:ro|:rw]
    #[clap(long, multiple_occurrences = true)]
    share: Vec<String>,
    
    /// Read-only filesystem (applies to block devices)
    #[clap(long)]
    read_only: bool,
}

fn main() -> Result<()> {
    // Initialize logging
    CombinedLogger::init(vec![
        TermLogger::new(
            LevelFilter::Info,
            Config::default(),
            TerminalMode::Mixed,
            simplelog::ColorChoice::Auto,
        ),
    ])
    .unwrap();

    // Parse command line arguments
    let args = Args::parse();
    
    info!("Starting Rust Hypervisor");
    info!("Kernel: {:?}", args.kernel);
    info!("vCPUs: {}, Memory: {}MB", args.cpus, args.memory);
    
    // Create and configure the virtual machine
    let vm = match VirtualMachine::new(args.cpus) {
        Ok(vm) => vm,
        Err(e) => {
            error!("Failed to create VM: {}", e);
            exit(1);
        }
    };
    
    // Add network device if requested
    if let (Some(addr), Some(peer)) = (args.net, args.peer) {
        vm.add_virtio_net(&addr, &peer)?;
    }
    
    // Add shared directories
    for share in &args.share {
        let parts: Vec<&str> = share.splitn(3, ':').collect();
        let host_path = Path::new(parts[0]);
        let guest_path = parts.get(1).unwrap_or(&"").to_string();
        let readonly = parts.get(2).map_or(false, |p| p == &"ro");
        
        // Use the guest path as the tag if provided, otherwise use the host path
        let tag = if guest_path.is_empty() {
            host_path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("shared")
                .to_string()
        } else {
            guest_path.clone()
        };
        
        vm.add_virtio_fs(&tag, host_path, readonly)?;
        
        info!("Shared directory: {} -> {} (read-only: {})", 
              host_path.display(), 
              if guest_path.is_empty() { "<default>" } else { &guest_path },
              readonly);
    }
    
    // Load the kernel
    if let Err(e) = vm.load_kernel(&args.kernel) {
        error!("Failed to load kernel: {}", e);
        exit(1);
    }
    
    // Run the VM
    if let Err(e) = vm.run() {
        error!("VM execution failed: {}", e);
        exit(1);
    }
    
    info!("VM execution completed successfully");
    Ok(())
}
