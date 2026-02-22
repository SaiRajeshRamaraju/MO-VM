# MO-VM — Custom Virtual Machine in Rust

**A custom KVM-based virtual machine implemented in Rust with hardware-assisted virtualization, virtio devices, virtual disk management, and kernel boot integration.**

> ⚠️ **Status: Work in Progress** — Core hypervisor is functional. Some features (full 9P filesystem, advanced snapshotting,network routing) are under development.

---

## Overview

MO-VM is a fully featured virtual machine project designed to explore and implement low-level systems concepts, including:

- **Hardware-accelerated virtualization** via Linux KVM (using EPT/NPT for memory isolation)
- **Virtio device emulation** — block, console, network, filesystem
- **Virtual disk** with multi-level mapping, copy-on-write, deduplication, and AES/LUKS encryption
- **ELF kernel loading** — bootloader → kernel handoff
- **VM snapshot/restore** — save and restore complete VM state
- **Signal handling** — graceful shutdown on Ctrl+C

This project is written in Rust to leverage memory safety while maintaining low-level control over hardware.

---

## Features

### Hypervisor Core
- **KVM Backend**: Uses Linux KVM ioctls for hardware-accelerated virtualization
- **x86_64 Support**: Full x86_64 guest CPU initialization (registers, segments, MSRs, FPU)
- **ELF Kernel Loading**: Loads Linux kernels in ELF format into guest memory
- **Multi-vCPU**: Support for multiple virtual CPUs with scoped thread execution
- **Guest Memory**: Configurable guest physical memory with proper KVM memory slot registration

### Virtio Devices
- `virtio-blk` — Block device backed by a disk image file
- `virtio-console` — Console device connected to host stdout
- `virtio-net` — Network device with UDP socket transport
- `virtio-fs` — Filesystem sharing via 9P protocol (vhost-user backend)

### State Management
- **Snapshot/Restore**: Save and restore complete VM state (vCPUs, memory, devices) to JSON
- **Signal Handling**: Graceful shutdown on Ctrl+C via the `ctrlc` crate

### Virtual Disk (Future/Planned)
- L1/L2 mapping structure for efficient block management
- Copy-on-Write (COW) support for snapshotting
- Deduplication and encryption (AES/LUKS)
- Health checks and metadata storage

---

## Prerequisites

- **Linux host** with KVM support (`/dev/kvm` must be accessible)
- **Rust toolchain** >= 1.70 (stable, edition 2021)
- A **Linux kernel in ELF format** for booting

To check KVM support:
```bash
ls -la /dev/kvm
# If missing, enable KVM in your BIOS and load the kvm module:
# sudo modprobe kvm_intel  # or kvm_amd
```

---

## Building

```bash
git clone https://github.com/Ramarajusairajesh/MO-VM.git
cd MO-VM/rust-hypervisor
cargo build --release
```

The binary will be at `target/release/rust-hypervisor`.

---

## Usage

```bash
# Basic usage with a kernel
./target/release/rust-hypervisor --kernel path/to/vmlinuz

# Multiple vCPUs with more memory and a disk image
./target/release/rust-hypervisor \
  --kernel vmlinuz \
  --cpus 4 \
  --memory 512 \
  --disk rootfs.img

# With networking (UDP transport)
./target/release/rust-hypervisor \
  --kernel vmlinuz \
  --net 127.0.0.1:5000 \
  --peer 127.0.0.1:5001

# Share a host directory with the guest
./target/release/rust-hypervisor \
  --kernel vmlinuz \
  --share /path/to/shared/dir

# Save VM state on exit
./target/release/rust-hypervisor \
  --kernel vmlinuz \
  --save-state snapshot.json

# Restore from a previous snapshot
./target/release/rust-hypervisor \
  --kernel vmlinuz \
  --restore-state snapshot.json
```

---

## CLI Reference

| Option | Description | Default |
|--------|-------------|---------| 
| `-k, --kernel` | Path to Linux kernel (ELF) | `vmlinuz` |
| `-c, --cpus` | Number of vCPUs | `1` |
| `-m, --memory` | Memory size in MB | `128` |
| `-d, --disk` | Path to disk image | — |
| `--read-only` | Mount disk read-only | `false` |
| `--net` | Local UDP address for networking | — |
| `--peer` | Peer UDP address for networking | — |
| `--share` | Host directory to share via virtio-fs | — |
| `--save-state` | Save VM state on exit | — |
| `--restore-state` | Restore VM state before run | — |
| `--log-level` | Log level (error/warn/info/debug/trace) | `info` |

---

## Architecture

```
MO-VM/
└── rust-hypervisor/
    ├── src/
    │   ├── main.rs           ← CLI entry point & orchestration
    │   ├── lib.rs            ← Crate root (module declarations & re-exports)
    │   ├── vm.rs             ← VM lifecycle: KVM setup, vCPU threads, devices
    │   ├── vcpu.rs           ← vCPU init (registers, segments, MSRs) & run loop
    │   ├── memory.rs         ← Guest physical memory (mmap + KVM slots)
    │   ├── kernel_loader.rs  ← ELF parser, loads PT_LOAD segments to guest RAM
    │   ├── snapshot.rs       ← VM state serialization (serde for KVM structs)
    │   ├── error.rs          ← Unified HypervisorError enum
    │   └── virtio/           ← Virtio device implementations
    │       ├── mod.rs        ← VirtioDevice trait & VirtioDeviceConfig
    │       ├── block.rs      ← Virtual disk (/dev/vda)
    │       ├── console.rs    ← Virtual console (/dev/hvc0 → stdout)
    │       ├── net.rs        ← Virtual NIC (UDP socket transport)
    │       └── fs.rs         ← Virtual filesystem (9P protocol + ACLs)
    ├── tests/
    │   └── integration_test.rs ← Integration tests
    └── Cargo.toml
```

### Internal Architecture

```
┌──────────────────────────────────────────────────┐
│                    main.rs                        │
│              (CLI & orchestration)                │
├──────────────────────────────────────────────────┤
│                     vm.rs                         │
│           (VM lifecycle management)               │
├──────────┬───────────┬───────────────────────────┤
│ vcpu.rs  │ memory.rs │   kernel_loader.rs         │
│ (vCPU    │ (guest    │   (ELF loader)             │
│  init &  │  physical │                            │
│  run)    │  memory)  │                            │
├──────────┴───────────┼───────────────────────────┤
│                      │       snapshot.rs           │
│   virtio/            │    (save/restore state)     │
│   ├── mod.rs (trait) │                            │
│   ├── block.rs       │                            │
│   ├── console.rs     │                            │
│   ├── net.rs         │                            │
│   └── fs.rs          │                            │
├──────────────────────┴───────────────────────────┤
│                    error.rs                       │
│              (error types & Result)               │
└──────────────────────────────────────────────────┘
```

### Module Descriptions

| Module | Purpose |
|--------|---------|
| `main.rs` | CLI argument parsing, logging setup, device attachment, VM run loop |
| `vm.rs` | Creates KVM VM, manages vCPU threads, device registry, snapshot coordination |
| `vcpu.rs` | x86_64 CPU initialization (registers, segments, MSRs), KVM_RUN loop |
| `memory.rs` | Guest physical memory allocation (mmap) and KVM memory slot registration |
| `kernel_loader.rs` | Parses ELF files and loads PT_LOAD segments into guest memory |
| `snapshot.rs` | Serializes/deserializes VM state (vCPU registers + memory) via serde |
| `virtio/mod.rs` | `VirtioDevice` trait definition and `VirtioDeviceConfig` struct |
| `virtio/block.rs` | Virtual disk — reads/writes sectors from a host file |
| `virtio/console.rs` | Virtual serial console — forwards guest output to host stdout |
| `virtio/net.rs` | Virtual NIC — sends/receives Ethernet frames via UDP sockets |
| `virtio/fs.rs` | Virtual filesystem — shares a host directory via 9P protocol |
| `error.rs` | Unified `HypervisorError` enum with `From` conversions for all error types |

### Key Dependencies

| Crate | Purpose |
|-------|---------|
| `kvm-ioctls` | Safe Rust wrappers for KVM ioctls |
| `kvm-bindings` | KVM struct definitions (kvm_regs, kvm_sregs, etc.) |
| `vm-memory` | Guest memory abstractions (mmap, GuestAddress) |
| `virtio-queue` | Virtio queue implementation (descriptor chains, used ring) |
| `vhost-user-backend` | Vhost-user backend trait for virtio-fs |
| `clap` | Command-line argument parsing |
| `serde` + `serde_json` | Serialization for VM snapshots |
| `goblin` | ELF binary parser |
| `thiserror` | Derive macro for Error trait |

---

## Running Tests

```bash
cd rust-hypervisor
cargo test
```

Tests include:
- Snapshot serialization/deserialization round-trip
- File operations (create, read, write, delete)
- Directory operations (create, list, remove)
- ACL permission checks
- MMIO read/write and bounds checking
- VirtioFs device type and feature queries
- Concurrent file access

---

## Related Projects

- **32-bit custom kernel** with keyboard input (C)
- **eBPF-based packet filtering tool** (C + Go)
- **16-bit boot loader** — BIOS/MBR bootloader (yeah, BIOS rather than UEFI — UEFI firmware can still boot from MBR-partitioned disks using legacy BIOS compatibility mode / CSM)
- **Distributed chunk file storage** with metadata in Redis (C++)

These projects showcase experience across kernel-level development, low-level networking, and distributed systems.

---

## Contributing

This is a personal research project. Contributions or feedback from systems engineers are welcome, especially regarding:
- MMIO design and device emulation
- Memory partitioning and EPT/NPT optimization
- Virtual device handling and interrupt routing
- 9P protocol implementation

---

## License

MIT
