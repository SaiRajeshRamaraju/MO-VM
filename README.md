# MO-VM [Still a work in progress not completed]

**A custom virtual machine implemented in Rust with hardware-assisted memory partitioning, virtual disk management, and kernel boot integration.**

---

## Overview

MO-VM is a fully featured virtual machine project designed to explore and implement low-level systems concepts, including:

* Memory partitioning with hardware-enforced isolation (EPT/NPT)
* Virtual disk with multi-level mapping, copy-on-write, deduplication, snapshots, and AES/LUKS encryption
* OS bootloader → kernel handoff
* Future MMIO and device integration

This project is written in Rust to leverage memory safety while maintaining low-level control.

---

## Architecture

### 1. Virtual Disk

* L1/L2 mapping structure for efficient block management
* Copy-on-Write (COW) support for snapshotting
* Deduplication and encryption (AES/LUKS)
* Health checks and metadata storage
* Diagram placeholder: `virtual_disk_structure.png`

### 2. Boot Flow

* Bootloader loads the custom 32-bit kernel
* Kernel initialization includes device setup and memory mapping
* Flowchart placeholder: `boot_flow.png`

### 3. MMIO & Device Integration (Future Work)

* Plans to add memory-mapped I/O support
* Virtual device registration and interrupt handling
* Diagram placeholder: `mmio_design.png`

---

## Key Features Implemented

* Hardware-assisted memory isolation (EPT/NPT)
* Virtual networking and partitioning
* Multi-level virtual disk with snapshot, COW, and deduplication
* AES/LUKS encrypted storage
* OS bootloader → kernel handoff with deterministic initialization

---

## Getting Started

### Prerequisites

* Rust >= 1.70
* Linux host with KVM enabled

### Build & Run

```bash
git clone https://github.com/Ramarajusairajesh/MO-VM.git
cd MO-VM
cargo build --release
./target/release/mo-vm --disk path/to/disk.img
```

---


## Related Projects

* 32-bit custom kernel with keyboard input (C)
* eBPF-based packet filtering tool (C + Go)
* 16-bit boot loader. (Yeah, BIOS rather than UEFI. UEFI firmware can still boot from MBR-partitioned disks using legacy BIOS compatibility mode (CSM), which reads the first sector (the MBR).)
* Distributed chunk file storage with metadata in Redis (C++)

These projects showcase experience across kernel-level development, low-level networking, and distributed systems.

---

## Contributing

This is a personal research project. Contributions or feedback from systems engineers are welcome, especially regarding MMIO design, memory partitioning, and virtual device handling.

---

## License

MIT License

