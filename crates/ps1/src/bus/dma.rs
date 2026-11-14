//! DMA channel execution, bounded GPU lists, MDEC backpressure and interrupt latching.
use super::{Bus, MdecTransfer, RAM_SIZE, physical};
use crate::EmuError;

impl Bus {
    pub(super) fn write_dma_lanes(
        &mut self,
        address: u32,
        value: u32,
        mask: u32,
    ) -> Result<(), EmuError> {
        let address = physical(address);
        let shift = (address & 3) * 8;
        let mask = mask << shift;
        let value = value << shift;
        if address & !3 == 0x1f80_10f4 {
            self.write_dicr(value, mask);
            Ok(())
        } else {
            let old = self.read32(address & !3)?;
            self.write32(address & !3, (old & !mask) | (value & mask))
        }
    }

    pub(super) fn write_dicr(&mut self, value: u32, mask: u32) {
        let flags = (self.dma_interrupt & 0x7f00_0000) & !(value & mask);
        let control = ((self.dma_interrupt & !mask) | (value & mask)) & 0x00ff_807f;
        self.dma_interrupt = (self.dma_interrupt & 0x8000_0000) | flags | control;
        self.update_dma_irq();
    }

    fn update_dma_irq(&mut self) {
        // DICR's master flag latches I_STAT only on a rising edge.
        // https://psx-spx.consoledev.net/dmachannels/#1f8010f4h-dicr-dma-interrupt-register-rw
        let was_active = self.dma_interrupt & 0x8000_0000 != 0;
        let forced = self.dma_interrupt & (1 << 15) != 0;
        let enabled = self.dma_interrupt & (1 << 23) != 0;
        let pending = self.dma_interrupt & 0x7f00_0000 != 0;
        let active = forced || (enabled && pending);
        self.dma_interrupt = (self.dma_interrupt & 0x7fff_ffff) | (u32::from(active) << 31);
        if active && !was_active {
            self.irq_status |= 8;
        }
    }

    fn ram_word(&self, address: u32) -> u32 {
        let index = address as usize & 0x1f_fffc;
        u32::from_le_bytes(
            self.ram[index..index + 4]
                .try_into()
                .expect("aligned RAM word"),
        )
    }

    fn set_ram_word(&mut self, address: u32, value: u32) {
        let index = address as usize & 0x1f_fffc;
        self.ram[index..index + 4].copy_from_slice(&value.to_le_bytes());
    }

    #[allow(clippy::too_many_lines)] // Keep the transfer-mode dispatcher together.
    pub(super) fn run_dma(&mut self, index: usize) -> Result<(), EmuError> {
        let channel = self.dma[index];
        if self.dma_control & (8 << (index * 4)) == 0 || channel.control & (1 << 24) == 0 {
            return Ok(());
        }
        let sync = (channel.control >> 9) & 3;
        if sync == 0
            && channel.control & (1 << 28) == 0
            && !(index == 3 && self.cdrom.data_available() > 0)
            && !(index == 4 && self.spu.dma_ready(channel.control & 1 != 0))
        {
            return Ok(());
        }
        let from_ram = channel.control & 1 != 0;
        let chopping = channel.control & (1 << 8) != 0;
        if chopping && (index != 3 || sync != 0 || from_ram) {
            return Err(EmuError::Dma("chopping requires CD-ROM input"));
        }
        if index == 3 {
            if self.cdrom_dma_resume.is_some_and(|resume| {
                // Chopping CPU windows are at most 128 cycles. A larger
                // wrapping difference denotes a deadline already in the past.
                (1..=128).contains(&resume.wrapping_sub(self.elapsed_cycles))
            }) {
                return Ok(());
            }
            self.cdrom_dma_resume = None;
        }
        if index < 2 {
            if sync != 1 || from_ram != (index == 0) || channel.control & 2 != 0 {
                return Err(EmuError::Dma(
                    "MDEC requires incrementing request-mode DMA0 input or DMA1 output",
                ));
            }
            if !self.run_mdec_dma(index)? {
                return Ok(());
            }
            self.finish_dma(index);
            return Ok(());
        }
        let mut address = channel.base & 0x1f_fffc;
        if sync == 2 {
            if index != 2 || !from_ram {
                return Err(EmuError::Dma("linked lists require RAM -> GPU channel 2"));
            }
            self.dma_visited.clear();
            let mut words = 0;
            loop {
                if !self.dma_visited.insert(address) {
                    // No end marker was reached. Keep the channel busy so
                    // software can abort it, but bound this atomic traversal.
                    self.dma[index].base = address;
                    self.gpu.dispatch_pending();
                    return Ok(());
                }
                let header = self.ram_word(address);
                for _ in 0..header >> 24 {
                    words += 1;
                    if words > 65_536 {
                        return Err(EmuError::Dma(
                            "linked list exceeds prototype transfer limit",
                        ));
                    }
                    address = address.wrapping_add(4) & 0x1f_fffc;
                    self.gpu.gp0(self.ram_word(address))?;
                }
                if header & 0x0080_0000 != 0 {
                    break;
                }
                address = header & 0x1f_fffc;
            }
            self.dma[index].base = 0x00ff_ffff;
        } else {
            if sync > 1
                || !matches!((index, from_ram), (2 | 4, _) | (3 | 6, false))
                || (matches!(index, 3 | 6) && sync != 0)
            {
                return Err(EmuError::Dma(
                    "only GPU, SPU, manual CD-ROM input, and manual OTC transfers are implemented",
                ));
            }
            let block_size = match channel.block & 0xffff {
                0 => 0x1_0000,
                n => n,
            };
            let blocks = if sync == 1 {
                match channel.block >> 16 {
                    0 => 0x1_0000,
                    n => n,
                }
            } else {
                1
            };
            let count = u64::from(block_size) * u64::from(blocks);
            if count > (RAM_SIZE / 4) as u64 {
                return Err(EmuError::Dma("transfer exceeds prototype RAM-sized limit"));
            }
            if index == 3 {
                if self.cdrom.data_available() == 0 {
                    return Ok(());
                }
                if self.cdrom.data_available() < count as usize * 4 {
                    return Err(EmuError::Dma(
                        "CD-ROM DMA exceeds the available sector data",
                    ));
                }
            }
            if index == 4 && !self.spu.dma_ready(from_ram) {
                return Ok(());
            }
            let count = if chopping {
                count.min(1 << ((channel.control >> 16) & 7))
            } else {
                count
            };
            let decrement = index == 6 || channel.control & 2 != 0;
            for n in 0..count {
                if index == 6 {
                    self.set_ram_word(
                        address,
                        if n + 1 == count {
                            0x00ff_ffff
                        } else {
                            address.wrapping_sub(4) & 0x1f_ffff
                        },
                    );
                } else if index == 3 {
                    let value = u32::from_le_bytes(std::array::from_fn(|_| self.cdrom.read_data()));
                    self.set_ram_word(address, value);
                } else if index == 4 {
                    if from_ram {
                        self.spu.dma_write(self.ram_word(address))?;
                    } else {
                        let word = self.spu.dma_read()?;
                        self.set_ram_word(address, word);
                    }
                } else if from_ram {
                    self.gpu.gp0(self.ram_word(address))?;
                } else {
                    let value = self.gpu.read();
                    self.set_ram_word(address, value);
                }
                address = if decrement {
                    address.wrapping_sub(4)
                } else {
                    address.wrapping_add(4)
                } & 0x1f_fffc;
            }
            // Unlike ordinary CD bursts, chopping exposes the next address
            // and remaining word count during each CPU window.
            if index != 3 || chopping {
                self.dma[index].base = address;
            }
            if chopping {
                let remaining = block_size - count as u32;
                self.dma[index].block = (channel.block & 0xffff_0000) | (remaining & 0xffff);
                self.dma[index].control &= !(1 << 28);
                if remaining != 0 {
                    self.cdrom_dma_resume = Some(
                        self.elapsed_cycles
                            .wrapping_add(1 << ((channel.control >> 20) & 7)),
                    );
                    return Ok(());
                }
            }
            if sync == 1 {
                self.dma[index].block &= 0xffff;
            }
        }
        self.finish_dma(index);
        Ok(())
    }

    fn finish_dma(&mut self, index: usize) {
        if index == 2 {
            self.gpu.dispatch_pending();
        }
        self.dma[index].control &= !((1 << 24) | (1 << 28));
        if self.dma_interrupt & ((1 << 23) | (1 << (16 + index)))
            == ((1 << 23) | (1 << (16 + index)))
        {
            self.dma_interrupt |= 1 << (24 + index);
        }
        self.update_dma_irq();
        if self.spu.irq_pending() {
            self.irq_status |= 1 << 9;
        }
        if self.gpu.irq {
            self.irq_status |= 2;
        }
    }

    fn run_mdec_dma(&mut self, index: usize) -> Result<bool, EmuError> {
        let channel = self.dma[index];
        let mut transfer = if let Some(progress) = self.mdec_dma[index] {
            progress
        } else {
            let block_size = match channel.block & 0xffff {
                0 => 0x1_0000,
                n => n,
            };
            let blocks = match channel.block >> 16 {
                0 => 0x1_0000,
                n => n,
            };
            if u64::from(block_size) * u64::from(blocks) > (RAM_SIZE / 4) as u64 {
                return Err(EmuError::Dma("MDEC transfer exceeds RAM-sized limit"));
            }
            MdecTransfer {
                address: channel.base & 0x1f_fffc,
                remaining: block_size * blocks,
                block_size,
            }
        };
        while transfer.remaining != 0 {
            if index == 0 {
                if !self.mdec.dma_input_ready() {
                    break;
                }
                self.mdec.write_data(self.ram_word(transfer.address))?;
            } else {
                if !self.mdec.dma_output_ready() {
                    break;
                }
                let (word, offset) = self.mdec.read_dma();
                self.set_ram_word(transfer.address.wrapping_add_signed(offset), word);
            }
            transfer.address = transfer.address.wrapping_add(4) & 0x1f_fffc;
            transfer.remaining -= 1;
        }
        self.dma[index].base = transfer.address;
        self.dma[index].block =
            (transfer.remaining.div_ceil(transfer.block_size) << 16) | (channel.block & 0xffff);
        self.mdec_dma[index] = (transfer.remaining != 0).then_some(transfer);
        Ok(transfer.remaining == 0)
    }
}
