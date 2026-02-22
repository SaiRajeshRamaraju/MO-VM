use vm_memory::MemoryRegionAddress;
use std::fs::File;
use std::io;
use log::info;
use std::fmt;
use std::path::Path;
use std::sync::Arc;
use serde::{Deserialize, Serialize, Deserializer, Serializer};
use serde::de::{self, MapAccess, Visitor};
use serde::ser::{SerializeStruct, SerializeSeq};
use serde_bytes;
use vm_memory::{Bytes, GuestAddress, GuestMemory, GuestMemoryMmap, GuestMemoryRegion};
use kvm_ioctls::VmFd;

// Use the standard Result type to avoid conflicts with serde's expected Result
type Result<T, E = Box<dyn std::error::Error + Send + Sync + 'static>> = std::result::Result<T, E>;

// Custom serialization for kvm_regs
fn serialize_kvm_regs<S>(regs: &kvm_bindings::kvm_regs, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let mut state = serializer.serialize_struct("kvm_regs", 20)
        .map_err(|e| serde::ser::Error::custom(e))?;
    
    macro_rules! serialize_field {
        ($state:ident, $field:ident) => {
            $state.serialize_field(stringify!($field), &regs.$field)
                .map_err(|e| serde::ser::Error::custom(e))?;
        };
    }
   // Serialize each of the kvm regs ?But why
    serialize_field!(state, rax);
    serialize_field!(state, rbx);
    serialize_field!(state, rcx);
    serialize_field!(state, rdx);
    serialize_field!(state, rsi);
    serialize_field!(state, rdi);
    serialize_field!(state, rsp);
    serialize_field!(state, rbp);
    serialize_field!(state, r8);
    serialize_field!(state, r9);
    serialize_field!(state, r10);
    serialize_field!(state, r11);
    serialize_field!(state, r12);
    serialize_field!(state, r13);
    serialize_field!(state, r14);
    serialize_field!(state, r15);
    serialize_field!(state, rip);
    serialize_field!(state, rflags);
    
    state.end()
}

fn deserialize_kvm_regs<'de, D>(deserializer: D) -> Result<kvm_bindings::kvm_regs, D::Error>
where
    D: Deserializer<'de>,
{
    // Using a custom visitor to handle kvm_regs deserialization
    struct RegsVisitor;

    impl<'de> Visitor<'de> for RegsVisitor {
        type Value = kvm_bindings::kvm_regs;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("struct kvm_regs")
        }
        
        fn visit_map<A>(self, mut map: A) -> std::result::Result<Self::Value, A::Error>
        where
            A: de::MapAccess<'de>,
        {
            let mut regs: kvm_bindings::kvm_regs = unsafe { std::mem::zeroed() };

            while let Some(key) = map.next_key::<String>()? {
                match key.as_str() {
                    "rax" => regs.rax = map.next_value()?,
                    "rbx" => regs.rbx = map.next_value()?,
                    "rcx" => regs.rcx = map.next_value()?,
                    "rdx" => regs.rdx = map.next_value()?,
                    "rsi" => regs.rsi = map.next_value()?,
                    "rdi" => regs.rdi = map.next_value()?,
                    "rsp" => regs.rsp = map.next_value()?,
                    "rbp" => regs.rbp = map.next_value()?,
                    "r8" => regs.r8 = map.next_value()?,
                    "r9" => regs.r9 = map.next_value()?,
                    "r10" => regs.r10 = map.next_value()?,
                    "r11" => regs.r11 = map.next_value()?,
                    "r12" => regs.r12 = map.next_value()?,
                    "r13" => regs.r13 = map.next_value()?,
                    "r14" => regs.r14 = map.next_value()?,
                    "r15" => regs.r15 = map.next_value()?,
                    "rip" => regs.rip = map.next_value()?,
                    "rflags" => regs.rflags = map.next_value()?,
                    _ => {
                        // Skip unknown fields
                        let _: de::IgnoredAny = map.next_value()?;
                    }
                }
            }

            Ok(regs)
        }
    }

    deserializer.deserialize_map(RegsVisitor)
}

// Custom serialization for kvm_sregs
fn serialize_kvm_sregs<S>(sregs: &kvm_bindings::kvm_sregs, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    // For simplicity, we'll serialize the sregs as raw bytes
    let bytes = unsafe {
        std::slice::from_raw_parts(
            sregs as *const _ as *const u8,
            std::mem::size_of::<kvm_bindings::kvm_sregs>()
        )
    };
    serializer.serialize_bytes(bytes)
        .map_err(serde::ser::Error::custom)
}

fn deserialize_kvm_sregs<'de, D>(deserializer: D) -> Result<kvm_bindings::kvm_sregs, D::Error>
where
    D: Deserializer<'de>,
{
    let bytes = <&[u8]>::deserialize(deserializer)
        .map_err(serde::de::Error::custom)?;
        
    if bytes.len() != std::mem::size_of::<kvm_bindings::kvm_sregs>() {
        return Err(serde::de::Error::invalid_length(
            bytes.len(), 
            &format!("kvm_sregs (expected {} bytes)", std::mem::size_of::<kvm_bindings::kvm_sregs>()).as_str()
        ));
    }
    
    let mut sregs = kvm_bindings::kvm_sregs::default();
    unsafe {
        std::ptr::copy_nonoverlapping(
            bytes.as_ptr(),
            &mut sregs as *mut _ as *mut u8,
            std::mem::size_of::<kvm_bindings::kvm_sregs>()
        );
    }
    
    Ok(sregs)
}

// Custom serialization for kvm_fpu
fn serialize_kvm_fpu<S>(fpu: &kvm_bindings::kvm_fpu, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    // For simplicity, we'll serialize the fpu as raw bytes
    let bytes = unsafe {
        std::slice::from_raw_parts(
            fpu as *const _ as *const u8,
            std::mem::size_of::<kvm_bindings::kvm_fpu>()
        )
    };
    serializer.serialize_bytes(bytes)
        .map_err(serde::ser::Error::custom)
}

fn deserialize_kvm_fpu<'de, D>(deserializer: D) -> Result<kvm_bindings::kvm_fpu, D::Error>
where
    D: Deserializer<'de>,
{
    let bytes = <&[u8]>::deserialize(deserializer)
        .map_err(serde::de::Error::custom)?;
        
    if bytes.len() != std::mem::size_of::<kvm_bindings::kvm_fpu>() {
        return Err(serde::de::Error::invalid_length(
            bytes.len(), 
            &format!("kvm_fpu (expected {} bytes)", std::mem::size_of::<kvm_bindings::kvm_fpu>()).as_str()
        ));
    }
    
    let mut fpu = kvm_bindings::kvm_fpu::default();
    unsafe {
        std::ptr::copy_nonoverlapping(
            bytes.as_ptr(),
            &mut fpu as *mut _ as *mut u8,
            std::mem::size_of::<kvm_bindings::kvm_fpu>()
        );
    }
    
    Ok(fpu)
}

// Custom serialization for kvm_msr_entry
fn serialize_kvm_msr_entry<S>(entry: &kvm_bindings::kvm_msr_entry, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let mut state = serializer.serialize_struct("kvm_msr_entry", 3)
        .map_err(serde::ser::Error::custom)?;
        
    state.serialize_field("index", &entry.index)
        .map_err(serde::ser::Error::custom)?;
    state.serialize_field("reserved", &entry.reserved)
        .map_err(serde::ser::Error::custom)?;
    state.serialize_field("data", &entry.data)
        .map_err(serde::ser::Error::custom)?;
        
    state.end()
}

fn deserialize_kvm_msr_entry<'de, D>(deserializer: D) -> Result<kvm_bindings::kvm_msr_entry, D::Error>
where
    D: Deserializer<'de>,
{
    // Using a custom visitor to handle kvm_msr_entry deserialization
    struct MsrEntryVisitor;

    impl<'de> Visitor<'de> for MsrEntryVisitor {
        type Value = kvm_bindings::kvm_msr_entry;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("struct kvm_msr_entry")
        }
        
        fn visit_map<A>(self, mut map: A) -> std::result::Result<Self::Value, A::Error>
        where
            A: de::MapAccess<'de>,
        {
            let mut index = None;
            let mut reserved = None;
            let mut data = None;

            while let Some(key) = map.next_key::<String>()? {
                match key.as_str() {
                    "index" => {
                        if index.is_some() {
                            return Err(de::Error::duplicate_field("index"));
                        }
                        index = Some(map.next_value()?);
                    }
                    "reserved" => {
                        if reserved.is_some() {
                            return Err(de::Error::duplicate_field("reserved"));
                        }
                        reserved = Some(map.next_value()?);
                    }
                    "data" => {
                        if data.is_some() {
                            return Err(de::Error::duplicate_field("data"));
                        }
                        data = Some(map.next_value()?);
                    }
                    _ => {
                        // Skip unknown fields
                        let _: de::IgnoredAny = map.next_value()?;
                    }
                }
            }

            let index = index.ok_or_else(|| de::Error::missing_field("index"))?;
            let reserved = reserved.ok_or_else(|| de::Error::missing_field("reserved"))?;
            let data = data.ok_or_else(|| de::Error::missing_field("data"))?;

            Ok(kvm_bindings::kvm_msr_entry {
                index,
                reserved,
                data,
            })
        }
    }

    deserializer.deserialize_map(MsrEntryVisitor)
}

fn serialize_msr_entries<S>(msrs: &[kvm_bindings::kvm_msr_entry], serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let mut seq = serializer.serialize_seq(Some(msrs.len()))?;
    
    for entry in msrs {
        seq.serialize_element(&MsrEntrySerde {
            index: entry.index,
            reserved: entry.reserved,
            data: entry.data,
        })?;
    }
    
    seq.end()
}

#[derive(Debug)]
pub struct VmState {
    pub memory_regions: Vec<MemoryRegion>,
    pub vcpu_states: Vec<VcpuState>,
    pub device_states: Vec<DeviceState>,
}

impl Serialize for VmState {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("VmState", 3)?;
        state.serialize_field("memory_regions", &self.memory_regions)?;
        state.serialize_field("vcpu_states", &self.vcpu_states)?;
        state.serialize_field("device_states", &self.device_states)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for VmState {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(field_identifier, rename_all = "snake_case")]
        enum Field { 
            MemoryRegions, 
            DeviceStates, 
            VcpuStates,
            #[serde(other)]
            Ignored,
        }

        struct VmStateVisitor;

        impl<'de> Visitor<'de> for VmStateVisitor {
            type Value = VmState;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("struct VmState")
            }

            fn visit_map<V>(self, mut map: V) -> std::result::Result<VmState, V::Error>
            where
                V: MapAccess<'de>,
            {
                let mut memory_regions = None;
                let mut device_states = None;
                let mut vcpu_states = None;

                while let Some(key) = map.next_key()? {
                    match key {
                        Field::MemoryRegions => {
                            if memory_regions.is_some() {
                                return Err(de::Error::duplicate_field("memory_regions"));
                            }
                            memory_regions = Some(map.next_value()?);
                        }
                        Field::DeviceStates => {
                            if device_states.is_some() {
                                return Err(de::Error::duplicate_field("device_states"));
                            }
                            device_states = Some(map.next_value()?);
                        }
                        Field::VcpuStates => {
                            if vcpu_states.is_some() {
                                return Err(de::Error::duplicate_field("vcpu_states"));
                            }
                            vcpu_states = Some(map.next_value()?);
                        }
                        Field::Ignored => {
                            // Skip unknown fields
                            let _: de::IgnoredAny = map.next_value()?;
                        }
                    }
                }

                let memory_regions = memory_regions.ok_or_else(|| de::Error::missing_field("memory_regions"))?;
                let device_states = device_states.unwrap_or_default();
                let vcpu_states = vcpu_states.unwrap_or_default();

                Ok(VmState {
                    memory_regions,
                    device_states,
                    vcpu_states,
                })
            }
        }

        const FIELDS: &'static [&'static str] = &["memory_regions", "device_states", "vcpu_states"];
        deserializer.deserialize_struct("VmState", FIELDS, VmStateVisitor)
    }
}

// Ensure all fields implement the required traits
#[derive(Debug)]
pub struct MemoryRegion {
    pub guest_addr: u64,
    pub size: usize,
    pub data: Vec<u8>,
}

impl Serialize for MemoryRegion {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        
        
        let mut state = serializer.serialize_struct("MemoryRegion", 3)?;
        state.serialize_field("guest_addr", &self.guest_addr)?;
        state.serialize_field("size", &self.size)?;
        state.serialize_field("data", &self.data)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for MemoryRegion {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(field_identifier, rename_all = "snake_case")]
        enum Field { GuestAddr, Size, Data, #[serde(other)] Ignored }

        struct MemoryRegionVisitor;

        impl<'de> Visitor<'de> for MemoryRegionVisitor {
            type Value = MemoryRegion;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("struct MemoryRegion")
            }

            fn visit_map<V>(self, mut map: V) -> std::result::Result<MemoryRegion, V::Error>
            where
                V: MapAccess<'de>,
            {
                let mut guest_addr = None;
                let mut size = None;
                let mut data = None;

                while let Some(key) = map.next_key()? {
                    match key {
                        Field::GuestAddr => {
                            if guest_addr.is_some() {
                                return Err(de::Error::duplicate_field("guest_addr"));
                            }
                            guest_addr = Some(map.next_value()?);
                        }
                        Field::Size => {
                            if size.is_some() {
                                return Err(de::Error::duplicate_field("size"));
                            }
                            size = Some(map.next_value()?);
                        }
                        Field::Data => {
                            if data.is_some() {
                                return Err(de::Error::duplicate_field("data"));
                            }
                            let bytes: Vec<u8> = map.next_value()?;
                            data = Some(bytes);
                        }
                        Field::Ignored => {
                            let _: de::IgnoredAny = map.next_value()?;
                        }
                    }
                }

                let guest_addr = guest_addr.ok_or_else(|| de::Error::missing_field("guest_addr"))?;
                let size = size.ok_or_else(|| de::Error::missing_field("size"))?;
                let data = data.ok_or_else(|| de::Error::missing_field("data"))?;

                Ok(MemoryRegion {
                    guest_addr,
                    size,
                    data,
                })
            }
        }

        const FIELDS: &'static [&'static str] = &["guest_addr", "size", "data"];
        deserializer.deserialize_struct("MemoryRegion", FIELDS, MemoryRegionVisitor)
    }
}

#[derive(Debug)]
pub struct DeviceState {
    pub device_type: String,
    pub state: Vec<u8>,
}

impl Serialize for DeviceState {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        
        
        let mut state = serializer.serialize_struct("DeviceState", 2)?;
        state.serialize_field("device_type", &self.device_type)?;
        state.serialize_field("state", &serde_bytes::Bytes::new(&self.state))?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for DeviceState {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(field_identifier, rename_all = "snake_case")]
        enum Field { DeviceType, State, #[serde(other)] Ignored }

        struct DeviceStateVisitor;

        impl<'de> Visitor<'de> for DeviceStateVisitor {
            type Value = DeviceState;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("struct DeviceState")
            }

            fn visit_map<V>(self, mut map: V) -> std::result::Result<DeviceState, V::Error>
            where
                V: MapAccess<'de>,
            {
                let mut device_type = None;
                let mut state = None;

                while let Some(key) = map.next_key()? {
                    match key {
                        Field::DeviceType => {
                            if device_type.is_some() {
                                return Err(de::Error::duplicate_field("device_type"));
                            }
                            device_type = Some(map.next_value()?);
                        }
                        Field::State => {
                            if state.is_some() {
                                return Err(de::Error::duplicate_field("state"));
                            }
                            let bytes: &serde_bytes::Bytes = map.next_value()?;
                            state = Some(bytes.to_vec());
                        }
                        Field::Ignored => {
                            let _: de::IgnoredAny = map.next_value()?;
                        }
                    }
                }

                let device_type = device_type.ok_or_else(|| de::Error::missing_field("device_type"))?;
                let state = state.ok_or_else(|| de::Error::missing_field("state"))?;

                Ok(DeviceState {
                    device_type,
                    state,
                })
            }
        }

        const FIELDS: &'static [&'static str] = &["device_type", "state"];
        deserializer.deserialize_struct("DeviceState", FIELDS, DeviceStateVisitor)
    }
}

#[derive(Debug)]
pub struct VcpuState {
    pub id: u32,
    pub regs: kvm_bindings::kvm_regs,
    pub sregs: kvm_bindings::kvm_sregs,
    pub fpu: kvm_bindings::kvm_fpu,
    pub msrs: Vec<kvm_bindings::kvm_msr_entry>,
}

impl Serialize for VcpuState {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        
        
        let mut state = serializer.serialize_struct("VcpuState", 5)?;
        
        // Serialize the id field
        state.serialize_field("id", &self.id)?;
        
        // Use our custom serialization functions
        state.serialize_field("regs", &SerdeRegs::from_regs(&self.regs))?;
        state.serialize_field("sregs", &SerdeSregs::from_sregs(&self.sregs))?;
        state.serialize_field("fpu", &SerdeFpu::from_fpu(&self.fpu))?;
        
        // For MSR entries, we'll serialize them as a sequence of (index, value) pairs
        let msr_entries: Vec<(u32, u64)> = self.msrs.iter()
            .map(|entry| (entry.index, entry.data))
            .collect();
        state.serialize_field("msrs", &msr_entries)?;
        
        state.end()
    }
}

impl<'de> Deserialize<'de> for VcpuState {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(field_identifier, rename_all = "snake_case")]
        enum Field {
            Id,
            Regs,
            Sregs,
            Fpu,
            Msrs,
            #[serde(other)]
            Ignored,
        }

        struct VcpuStateVisitor;

        impl<'de> Visitor<'de> for VcpuStateVisitor {
            type Value = VcpuState;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("struct VcpuState")
            }

            fn visit_map<V>(self, mut map: V) -> std::result::Result<VcpuState, V::Error>
            where
                V: MapAccess<'de>,
            {
                let mut id = None;
                let mut regs = None;
                let mut sregs = None;
                let mut fpu = None;
                let mut msrs = None;

                while let Some(key) = map.next_key()? {
                    match key {
                        Field::Id => {
                            if id.is_some() {
                                return Err(de::Error::duplicate_field("id"));
                            }
                            id = Some(map.next_value()?);
                        }
                        Field::Regs => {
                            if regs.is_some() {
                                return Err(de::Error::duplicate_field("regs"));
                            }
                            let serde_regs: SerdeRegs = map.next_value()?;
                            regs = Some(serde_regs.into_regs());
                        }
                        Field::Sregs => {
                            if sregs.is_some() {
                                return Err(de::Error::duplicate_field("sregs"));
                            }
                            let serde_sregs: SerdeSregs = map.next_value()?;
                            sregs = Some(serde_sregs.into_sregs());
                        }
                        Field::Fpu => {
                            if fpu.is_some() {
                                return Err(de::Error::duplicate_field("fpu"));
                            }
                            let serde_fpu: SerdeFpu = map.next_value()?;
                            fpu = Some(serde_fpu.into_fpu());
                        }
                        Field::Msrs => {
                            if msrs.is_some() {
                                return Err(de::Error::duplicate_field("msrs"));
                            }
                            let msr_entries: Vec<(u32, u64)> = map.next_value()?;
                            msrs = Some(msr_entries.into_iter()
                                .map(|(index, data)| kvm_bindings::kvm_msr_entry {
                                    index,
                                    data,
                                    ..Default::default()
                                })
                                .collect());
                        }
                        Field::Ignored => {
                            // Skip unknown fields
                            let _: de::IgnoredAny = map.next_value()?;
                        }
                    }
                }

                let id = id.ok_or_else(|| de::Error::missing_field("id"))?;
                let regs = regs.ok_or_else(|| de::Error::missing_field("regs"))?;
                let sregs = sregs.ok_or_else(|| de::Error::missing_field("sregs"))?;
                let fpu = fpu.ok_or_else(|| de::Error::missing_field("fpu"))?;
                let msrs = msrs.unwrap_or_default();

                Ok(VcpuState {
                    id,
                    regs,
                    sregs,
                    fpu,
                    msrs,
                })
            }
        }

        const FIELDS: &'static [&'static str] = &["id", "regs", "sregs", "fpu", "msrs"];
        deserializer.deserialize_struct("VcpuState", FIELDS, VcpuStateVisitor)
    }
}

// Helper structs for serializing KVM types
#[derive(serde::Serialize, serde::Deserialize)]
struct SerdeRegs(#[serde(with = "serde_bytes")] Vec<u8>);

impl SerdeRegs {
    fn from_regs(regs: &kvm_bindings::kvm_regs) -> Self {
        let bytes = unsafe {
            std::slice::from_raw_parts(
                regs as *const _ as *const u8,
                std::mem::size_of::<kvm_bindings::kvm_regs>()
            )
        };
        SerdeRegs(bytes.to_vec())
    }

    fn into_regs(self) -> kvm_bindings::kvm_regs {
        unsafe {
            assert_eq!(self.0.len(), std::mem::size_of::<kvm_bindings::kvm_regs>());
            std::ptr::read(self.0.as_ptr() as *const kvm_bindings::kvm_regs)
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SerdeSregs(#[serde(with = "serde_bytes")] Vec<u8>);

impl SerdeSregs {
    fn from_sregs(sregs: &kvm_bindings::kvm_sregs) -> Self {
        let bytes = unsafe {
            std::slice::from_raw_parts(
                sregs as *const _ as *const u8,
                std::mem::size_of::<kvm_bindings::kvm_sregs>()
            )
        };
        SerdeSregs(bytes.to_vec())
    }

    fn into_sregs(self) -> kvm_bindings::kvm_sregs {
        unsafe {
            assert_eq!(self.0.len(), std::mem::size_of::<kvm_bindings::kvm_sregs>());
            std::ptr::read(self.0.as_ptr() as *const kvm_bindings::kvm_sregs)
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SerdeFpu(#[serde(with = "serde_bytes")] Vec<u8>);

impl SerdeFpu {
    fn from_fpu(fpu: &kvm_bindings::kvm_fpu) -> Self {
        let bytes = unsafe {
            std::slice::from_raw_parts(
                fpu as *const _ as *const u8,
                std::mem::size_of::<kvm_bindings::kvm_fpu>()
            )
        };
        SerdeFpu(bytes.to_vec())
    }

    fn into_fpu(self) -> kvm_bindings::kvm_fpu {
        unsafe {
            assert_eq!(self.0.len(), std::mem::size_of::<kvm_bindings::kvm_fpu>());
            std::ptr::read(self.0.as_ptr() as *const kvm_bindings::kvm_fpu)
        }
    }
}

// Helper struct for MSR entry serialization
#[derive(serde::Serialize, serde::Deserialize)]
struct MsrEntrySerde {
    index: u32,
    reserved: u32,
    data: u64,
}

struct MsrEntriesVisitor;

impl<'de> Visitor<'de> for MsrEntriesVisitor {
    type Value = Vec<kvm_bindings::kvm_msr_entry>;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("sequence of kvm_msr_entry")
    }
    
    fn visit_seq<A>(self, mut seq: A) -> std::result::Result<Self::Value, A::Error>
    where
        A: de::SeqAccess<'de>,
    {
        let mut entries = Vec::new();
        while let Some(entry) = seq.next_element::<MsrEntrySerde>()? {
            entries.push(kvm_bindings::kvm_msr_entry {
                index: entry.index,
                reserved: entry.reserved,
                data: entry.data,
            });
        }
        Ok(entries)
    }
}

fn deserialize_msr_entries<'de, D>(deserializer: D) -> Result<Vec<kvm_bindings::kvm_msr_entry>, D::Error>
where
    D: Deserializer<'de>,
{
    deserializer.deserialize_seq(MsrEntriesVisitor)
}

pub struct SnapshotManager {
    memory: Arc<GuestMemoryMmap>,
    vm_fd: VmFd,
}

impl SnapshotManager {
    pub fn new(memory: Arc<GuestMemoryMmap>, vm_fd: VmFd) -> Self {
        Self { memory, vm_fd }
    }

    pub fn create_snapshot(&self, vcpus: &[crate::vcpu::Vcpu]) -> Result<VmState> {
        info!("Creating VM snapshot...");
        
        // Save memory regions
        let memory_regions = self.save_memory()?;
        
        // Save VCPU states
        let vcpu_states = vcpus
            .iter()
            .map(|vcpu| self.save_vcpu_state(vcpu))
            .collect::<Result<Vec<_>>>()?;
        
        // Save device states (simplified)
        let device_states = Vec::new(); // In a real implementation, save device states
        
        Ok(VmState {
            memory_regions,
            vcpu_states,
            device_states,
        })
    }
    
    pub fn restore_snapshot(&self, state: &VmState, vcpus: &mut [crate::vcpu::Vcpu]) -> Result<()> {
        info!("Restoring VM from snapshot...");
        
        // Restore memory
        self.restore_memory(&state.memory_regions)?;
        
        // Restore VCPUs
        for (i, vcpu_state) in state.vcpu_states.iter().enumerate() {
            if let Some(vcpu) = vcpus.get_mut(i) {
                self.restore_vcpu_state(vcpu, vcpu_state)?;
            }
        }
        
        // Restore devices (simplified)
        
        Ok(())
    }
    
    fn save_memory(&self) -> Result<Vec<MemoryRegion>> {
        let mut regions = Vec::new();
        
        // Get the memory regions from the guest memory
        // Get memory regions using the GuestMemory trait
        let mem_regions = self.memory.iter();
        for (_i, region) in mem_regions.enumerate() {
            let mem_size = region.len() as usize;
            let mut data = vec![0u8; mem_size];
            
            // Read the entire region in chunks
            for (i, chunk) in data.chunks_mut(4096).enumerate() {
                let offset = (i * 4096) as u64;
                let addr = MemoryRegionAddress(offset);
                region
                    .read_slice(chunk, addr)
                    .map_err(|e| crate::error::HypervisorError::MemoryError(
                        format!("Failed to read memory at 0x{:x}: {}", offset, e)
                    ))?;
            }
                
            regions.push(MemoryRegion {
                guest_addr: region.start_addr().0,
                size: mem_size,
                data,
            });
        }
        
        Ok(regions)
    }
    
    fn restore_memory(&self, regions: &[MemoryRegion]) -> Result<()> {
        use vm_memory::GuestMemory;
        
        for mem_region in regions {
            // Find the target region that contains the guest address
            if let Some(region) = self.memory.find_region(GuestAddress(mem_region.guest_addr)) {
                let region_start = region.start_addr().0;
                let region_end = region_start.checked_add(region.len() as u64).ok_or_else(|| 
                    crate::error::HypervisorError::MemoryError("Region end overflow".to_string())
                )?;
                
                let mem_region_end = mem_region.guest_addr.checked_add(mem_region.size as u64).ok_or_else(||
                    crate::error::HypervisorError::MemoryError("Memory region end overflow".to_string())
                )?;
                
                // Check if the region is large enough
                if region_start > mem_region.guest_addr || region_end < mem_region_end {
                    return Err(Box::new(crate::error::HypervisorError::MemoryError(
                        format!("Memory region 0x{:x} size {} doesn't fit in guest memory", 
                        mem_region.guest_addr, mem_region.size)
                    )));
                }

                // Calculate the offset within the region
                let region_offset = mem_region.guest_addr - region_start;

                // Write the memory in chunks to handle large regions
                for (i, chunk) in mem_region.data.chunks(4096).enumerate() {
                    let offset = region_offset + (i * 4096) as u64;
                    let addr = mem_region.guest_addr + (i * 4096) as u64;
                    
                    region
                        .write_slice(chunk, MemoryRegionAddress(offset))
                        .map_err(|e| crate::error::HypervisorError::MemoryError(
                            format!("Failed to write memory at 0x{:x}: {}", addr, e)
                        ))?;
                }
            } else {
                return Err(Box::new(crate::error::HypervisorError::MemoryError(
                    format!("No memory region found for guest address 0x{:x}", mem_region.guest_addr)
                )));
            }
        }
        
        Ok(())
    }
    
    fn save_vcpu_state(&self, vcpu: &crate::vcpu::Vcpu) -> Result<VcpuState> {
        let regs = vcpu.fd.get_regs()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
            
        let sregs = vcpu.fd.get_sregs()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
            
        let fpu = vcpu.fd.get_fpu()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
            
        // Get MSRs (simplified)
        let msrs = Vec::new();
        
        Ok(VcpuState {
            id: vcpu.id as u32,
            regs,
            sregs,
            fpu,
            msrs,
        })
    }
    
    fn restore_vcpu_state(&self, vcpu: &mut crate::vcpu::Vcpu, state: &VcpuState) -> Result<()> {
        vcpu.fd.set_regs(&state.regs)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
            
        vcpu.fd.set_sregs(&state.sregs)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
            
        vcpu.fd.set_fpu(&state.fpu)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
            
        // Restore MSRs (simplified)
        
        Ok(())
    }
    
    pub fn save_to_file(&self, path: &Path, vcpus: &[crate::vcpu::Vcpu]) -> Result<()> {
        let state = self.create_snapshot(vcpus)?;
        let file = File::create(path)?;
        bincode::serialize_into(file, &state)?;
        info!("Saved VM state to {:?}", path);
        Ok(())
    }
    
    pub fn load_from_file(
        &self,
        path: &Path,
        vcpus: &mut [crate::vcpu::Vcpu],
    ) -> Result<()> {
        let file = File::open(path)?;
        let state: VmState = bincode::deserialize_from(file)?;
        self.restore_snapshot(&state, vcpus)?;
        info!("Loaded VM state from {:?}", path);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use vm_memory::{Bytes, GuestAddress};

    #[test]
    fn test_snapshot() {
        // This is a basic test to verify serialization/deserialization
        let state = VmState {
            memory_regions: vec![
                MemoryRegion {
                    guest_addr: 0x1000,
                    size: 4096,
                    data: vec![1, 2, 3, 4],
                },
            ],
            vcpu_states: vec![VcpuState {
                id: 0,
                regs: Default::default(),
                sregs: Default::default(),
                fpu: Default::default(),
                msrs: Vec::new(),
            }],
            device_states: Vec::new(),
        };

        // Test serialization to JSON (custom Deserialize impl expects map format)
        let json = serde_json::to_string(&state).unwrap();
        
        // Test deserialization from JSON
        let deserialized: VmState = serde_json::from_str(&json).unwrap();
        
        // Verify key fields survived the round-trip
        assert_eq!(deserialized.memory_regions.len(), 1);
        assert_eq!(deserialized.memory_regions[0].guest_addr, 0x1000);
        assert_eq!(deserialized.memory_regions[0].size, 4096);
        assert_eq!(deserialized.vcpu_states.len(), 1);
        assert_eq!(deserialized.vcpu_states[0].id, 0);
        assert_eq!(deserialized.device_states.len(), 0);
        
        // Also test file-based round-trip
        let temp_dir = tempdir().unwrap();
        let path = temp_dir.path().join("snapshot.json");
        
        let file = File::create(&path).unwrap();
        serde_json::to_writer(file, &state).unwrap();
        
        let file = File::open(&path).unwrap();
        let loaded: VmState = serde_json::from_reader(file).unwrap();
        assert_eq!(loaded.memory_regions.len(), 1);
        assert_eq!(loaded.vcpu_states.len(), 1);
    }
}
