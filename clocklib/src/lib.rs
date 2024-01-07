#![no_std]

use bitvec::prelude::*;
use core::fmt::Debug;
use embedded_hal::blocking::i2c;
use is31fl3731_driver::{Error, IS31FL3731};

pub struct ClockDisplay<I2C> {
    pub drivers: [Option<IS31FL3731<I2C>>; 3],
}

pub struct Segment {
    pub leds: &'static [u8],
}

// maps segments to leds on one driver
pub const SEGMENTS: [Segment; 44] = [
    Segment { leds: &[1, 2, 18] },
    Segment { leds: &[3, 19] },
    Segment { leds: &[4, 20] },
    Segment { leds: &[5, 6, 21] },
    Segment { leds: &[0] },
    Segment { leds: &[17, 34] },
    Segment { leds: &[35] },
    Segment { leds: &[36] },
    Segment { leds: &[22, 37] },
    Segment { leds: &[7] }, // 10
    Segment { leds: &[16] },
    Segment { leds: &[33, 50] },
    Segment { leds: &[51] },
    Segment { leds: &[52] },
    Segment { leds: &[38, 53] },
    Segment { leds: &[23] },
    Segment { leds: &[49] },
    Segment { leds: &[67] },
    Segment { leds: &[68] },
    Segment { leds: &[54] }, // 20
    Segment { leds: &[32, 48] },
    Segment {
        leds: &[65, 66, 82],
    },
    Segment {
        leds: &[69, 70, 85],
    },
    Segment { leds: &[39, 55] },
    Segment { leds: &[81] },
    Segment { leds: &[83] },
    Segment { leds: &[84] },
    Segment { leds: &[86] },
    Segment { leds: &[64] },
    Segment { leds: &[97, 98] }, // 30
    Segment { leds: &[99] },
    Segment { leds: &[100] },
    Segment { leds: &[101, 102] },
    Segment { leds: &[71] },
    Segment { leds: &[80] },
    Segment { leds: &[113, 114] },
    Segment { leds: &[115] },
    Segment { leds: &[116] },
    Segment { leds: &[117, 118] },
    Segment { leds: &[87] }, // 40
    Segment {
        leds: &[96, 129, 130],
    },
    Segment { leds: &[112, 131] },
    Segment { leds: &[119, 132] },
    Segment {
        leds: &[103, 133, 134],
    },
];

pub struct Symbol {
    mask: [u8; 6],
}

const DIGITS: [Symbol; 10] = [
    Symbol {
        mask: [0x2f, 0x49, 0x69, 0x29, 0x49, 0xf],
    }, // 0
    Symbol {
        mask: [0xc7, 0x30, 0x6, 0xc6, 0x30, 0x6],
    }, // 1
    Symbol {
        mask: [0xf, 0x41, 0x6e, 0x27, 0x8, 0xf],
    }, // 2
    Symbol {
        mask: [0xf, 0x41, 0x6e, 0xe, 0x41, 0xf],
    }, // 3
    Symbol {
        mask: [0x29, 0x49, 0x6f, 0xe, 0x41, 0x8],
    }, // 4
    Symbol {
        mask: [0x2f, 0x8, 0x67, 0xe, 0x41, 0xf],
    }, // 5
    Symbol {
        mask: [0x2f, 0x8, 0x67, 0x2f, 0x49, 0xf],
    }, // 6
    Symbol {
        mask: [0xf, 0x41, 0x4c, 0xc6, 0x30, 0x6],
    }, // 7
    Symbol {
        mask: [0x2f, 0x49, 0x6f, 0x2f, 0x49, 0xf],
    }, // 8
    Symbol {
        mask: [0x2f, 0x49, 0x6f, 0xe, 0x41, 0xf],
    }, // 9
];

const PROGRESS_LTR: [Symbol; 6] = [
    Symbol {
        mask: [0x10, 0x4, 0x10, 0x10, 0x4, 0x0],
    },
    Symbol {
        mask: [0x30, 0xc, 0x31, 0x31, 0xc, 0x0],
    },
    Symbol {
        mask: [0x70, 0x1c, 0x33, 0x73, 0x1c, 0x0],
    },
    Symbol {
        mask: [0xf0, 0x3c, 0x37, 0xf7, 0x3c, 0x0],
    },
    Symbol {
        mask: [0xf0, 0x7d, 0x7f, 0xff, 0x7d, 0x0],
    },
    Symbol {
        mask: [0xf0, 0xff, 0xff, 0xff, 0xff, 0x0],
    },
];

const CH_LTR: [Symbol; 2] = [
    Symbol {
        mask: [0x2f, 0x9, 0x21, 0x21, 0x48, 0xf],
    },
    Symbol {
        mask: [0x29, 0x49, 0x6f, 0x2f, 0x49, 0x9],
    },
];

impl<I2C, E> ClockDisplay<I2C>
where
    E: Debug,
    I2C: i2c::Read<Error = E> + i2c::Write<Error = E>,
{
    pub fn new(drivers: [Option<IS31FL3731<I2C>>; 3]) -> ClockDisplay<I2C> {
        ClockDisplay { drivers }
    }

    pub fn setup(&mut self) -> Result<(), Error<E>> {
        for driver in self.drivers.iter_mut().flatten() {
            driver.setup()?;
            driver.enable_leds(&[128, 135, 136, 143])?;
        }

        Ok(())
    }

    pub fn draw_segment(
        &mut self,
        sub_display: u8,
        segment_id: usize,
        color: u8,
    ) -> Result<(), Error<E>> {
        assert!(sub_display < 6);

        let segment = &SEGMENTS[segment_id];
        let driver_no = sub_display / 2;
        let sub_display = sub_display % 2;

        for &led in segment.leds {
            let driver = &mut self.drivers[driver_no as usize];
            if let Some(driver) = driver {
                driver.set_color_byte(led + 8 * sub_display, color).unwrap();
            }
        }

        Ok(())
    }

    pub fn draw_symbol(
        &mut self,
        sub_display: u8,
        symbol_id: usize,
        color: u8,
    ) -> Result<(), Error<E>> {
        assert!(sub_display < 4);

        let symbol = &DIGITS[symbol_id];
        let bits = symbol.mask.view_bits::<Lsb0>();
        for (i, bit) in bits.iter().enumerate() {
            if i < SEGMENTS.len() {
                if bit == true {
                    self.draw_segment(sub_display, i, color)?;
                } else {
                    self.draw_segment(sub_display, i, 0x00)?;
                }
            }
        }

        Ok(())
    }
    
    pub fn draw_CH(
        &mut self,
        sub_display: u8,
        symbol_id: usize,
        color: u8,
    ) -> Result<(), Error<E>> {
        assert!(sub_display < 4);

        let symbol = &CH_LTR[symbol_id];
        let bits = symbol.mask.view_bits::<Lsb0>();
        for (i, bit) in bits.iter().enumerate() {
            if i < SEGMENTS.len() {
                if bit == true {
                    self.draw_segment(sub_display, i, color)?;
                } else {
                    self.draw_segment(sub_display, i, 0x00)?;
                }
            }
        }

        Ok(())
    }
}
