//! Device-owned conversion of bus registers, DMA progress and attached devices.
use super::{BIOS_SIZE, Bus, DmaChannel, MdecTransfer, RAM_SIZE};
use crate::{
    cdrom::CdromV1, gpu::GpuV1, mdec::MdecV1, sio::SioV1, spu::SpuV1, state::v1::impl_serde_v1,
    timers::TimersV1,
};
use std::{collections::HashSet, sync::Arc};

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename = "DmaChannel")]
struct DmaChannelV1 {
    base: u32,
    block: u32,
    control: u32,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename = "MdecTransfer")]
struct MdecTransferV1 {
    address: u32,
    remaining: u32,
    block_size: u32,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename = "Bus")]
pub(crate) struct BusV1 {
    ram: Vec<u8>,
    bios: Arc<[u8]>,
    gpu: GpuV1,
    cdrom: CdromV1,
    spu: SpuV1,
    post: u8,
    timers: TimersV1,
    sio: SioV1,
    mdec: MdecV1,
    mdec_dma: [Option<MdecTransferV1>; 2],
    cdrom_dma_resume: Option<u64>,
    #[serde(with = "crate::state::array")]
    scratchpad: [u8; 1024],
    memory_control: [u32; 9],
    ram_size: u32,
    cache_control: u32,
    irq_status: u32,
    irq_mask: u32,
    dma: [DmaChannelV1; 7],
    dma_control: u32,
    dma_interrupt: u32,
    elapsed_cycles: u64,
}

impl From<DmaChannel> for DmaChannelV1 {
    fn from(value: DmaChannel) -> Self {
        Self {
            base: value.base,
            block: value.block,
            control: value.control,
        }
    }
}

impl From<DmaChannelV1> for DmaChannel {
    fn from(value: DmaChannelV1) -> Self {
        Self {
            base: value.base,
            block: value.block,
            control: value.control,
        }
    }
}

impl From<MdecTransfer> for MdecTransferV1 {
    fn from(value: MdecTransfer) -> Self {
        Self {
            address: value.address,
            remaining: value.remaining,
            block_size: value.block_size,
        }
    }
}

impl From<MdecTransferV1> for MdecTransfer {
    fn from(value: MdecTransferV1) -> Self {
        Self {
            address: value.address,
            remaining: value.remaining,
            block_size: value.block_size,
        }
    }
}

impl_serde_v1!(Bus, BusV1);

impl TryFrom<&Bus> for BusV1 {
    type Error = String;

    fn try_from(value: &Bus) -> Result<Self, Self::Error> {
        Ok(Self {
            ram: value.ram.clone(),
            bios: Arc::clone(&value.bios),
            gpu: (&value.gpu).try_into()?,
            cdrom: (&value.cdrom).into(),
            spu: (&value.spu).into(),
            post: value.post,
            timers: (&value.timers).into(),
            sio: (&value.sio).into(),
            mdec: (&value.mdec).into(),
            mdec_dma: value.mdec_dma.map(|transfer| transfer.map(Into::into)),
            cdrom_dma_resume: value.cdrom_dma_resume,
            scratchpad: value.scratchpad,
            memory_control: value.memory_control,
            ram_size: value.ram_size,
            cache_control: value.cache_control,
            irq_status: value.irq_status,
            irq_mask: value.irq_mask,
            dma: value.dma.map(Into::into),
            dma_control: value.dma_control,
            dma_interrupt: value.dma_interrupt,
            elapsed_cycles: value.elapsed_cycles,
        })
    }
}

impl TryFrom<BusV1> for Bus {
    type Error = String;

    fn try_from(value: BusV1) -> Result<Self, Self::Error> {
        if value.ram.len() != RAM_SIZE || value.bios.len() != BIOS_SIZE {
            return Err("Invalid saved RAM or BIOS size".into());
        }
        if value.mdec_dma.iter().flatten().any(|transfer| {
            !(1..=0x1_0000).contains(&transfer.block_size)
                || !(1..=RAM_SIZE as u32 / 4).contains(&transfer.remaining)
                || transfer.address & !0x1f_fffc != 0
        }) {
            return Err("Invalid saved DMA transfer".into());
        }
        Ok(Self {
            ram: value.ram,
            bios: value.bios,
            gpu: value.gpu.try_into()?,
            cdrom: value.cdrom.try_into()?,
            spu: value.spu.try_into()?,
            post: value.post,
            timers: value.timers.try_into()?,
            sio: value.sio.try_into()?,
            mdec: value.mdec.try_into()?,
            mdec_dma: value.mdec_dma.map(|transfer| transfer.map(Into::into)),
            cdrom_dma_resume: value.cdrom_dma_resume,
            scratchpad: value.scratchpad,
            memory_control: value.memory_control,
            ram_size: value.ram_size,
            cache_control: value.cache_control,
            irq_status: value.irq_status,
            irq_mask: value.irq_mask,
            dma: value.dma.map(Into::into),
            dma_control: value.dma_control,
            dma_interrupt: value.dma_interrupt,
            elapsed_cycles: value.elapsed_cycles,
            dma_visited: HashSet::new(),
        })
    }
}
