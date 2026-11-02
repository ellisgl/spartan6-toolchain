//! Serialize a prjcombine Spartan-6 `Bitstream` into configuration packets,
//! in the exact sequence ISE's bitgen uses (plain, uncompressed,
//! unencrypted). The sequence mirrors prjcombine's parser
//! (`prjcombine_spartan6::bitstream::parse`), which checks it strictly.
//!
//! Packet format (UG380): 16-bit words. Type 1 header `0x3000 | reg << 5 |
//! count` for writes of up to 31 words; type 2 (FDRI) `0x5060` followed by a
//! 32-bit word count. A 22-bit CRC covers every written (register, word) pair
//! except CRC, LOUT and CSBO; RCRC resets it, and each FDRI is followed by an
//! automatic CRC word pair.

use prjcombine_spartan6::bitstream::{BRAM_FRAME_LEN, Bitstream, FRAME_LEN, FrameAddr, Reg};

mod reg {
    pub const CRC: u16 = 0x00;
    pub const FAR: u16 = 0x01;
    pub const FDRI: u16 = 0x03;
    pub const CMD: u16 = 0x05;
    pub const CTL0: u16 = 0x06;
    pub const MASK: u16 = 0x07;
    pub const COR1: u16 = 0x0a;
    pub const COR2: u16 = 0x0b;
    pub const PWRDN: u16 = 0x0c;
    pub const FLR: u16 = 0x0d;
    pub const IDCODE: u16 = 0x0e;
    pub const TIMER: u16 = 0x0f;
    pub const HC_OPT: u16 = 0x10;
    pub const TESTMODE: u16 = 0x11;
    pub const GENERAL1: u16 = 0x13;
    pub const GENERAL2: u16 = 0x14;
    pub const GENERAL3: u16 = 0x15;
    pub const GENERAL4: u16 = 0x16;
    pub const GENERAL5: u16 = 0x17;
    pub const MODE: u16 = 0x18;
    pub const PU_GWE: u16 = 0x19;
    pub const PU_GTS: u16 = 0x1a;
    pub const CCLK_FREQ: u16 = 0x1c;
    pub const SEU_OPT: u16 = 0x1d;
    pub const RBCRC_SW: u16 = 0x1e;
    pub const EYE_MASK: u16 = 0x21;
}

mod cmd {
    pub const WCFG: u16 = 1;
    pub const DGHIGH: u16 = 3;
    pub const START: u16 = 5;
    pub const RCRC: u16 = 7;
    pub const GRESTORE: u16 = 10;
    pub const DESYNC: u16 = 13;
}

const NOP: u16 = 0x2000;

/// Same as prjcombine's `Crc` (packet_s3a.rs): rotates once per 22 bits.
struct Crc(u32);

impl Crc {
    fn update(&mut self, reg: u16, val: u16) {
        let data = (val as u32) | (reg as u32) << 16;
        self.0 <<= 1;
        if (self.0 & 0x400000) != 0 {
            self.0 ^= 0x409081;
        }
        self.0 ^= data;
    }
}

struct Writer {
    out: Vec<u8>,
    crc: Crc,
}

impl Writer {
    fn word(&mut self, w: u16) {
        self.out.extend_from_slice(&w.to_be_bytes());
    }

    fn nops(&mut self, n: usize) {
        for _ in 0..n {
            self.word(NOP);
        }
    }

    fn write(&mut self, reg: u16, vals: &[u16]) {
        assert!(vals.len() < 32);
        self.word(0x3000 | reg << 5 | vals.len() as u16);
        for &v in vals {
            if !matches!(reg, reg::CRC | 0x09 | 0x12) {
                self.crc.update(reg, v);
            }
            self.word(v);
        }
        if reg == reg::CMD && vals == [cmd::RCRC] {
            self.crc.0 = 0;
        }
    }

    fn write1(&mut self, reg: u16, val: u32) {
        self.write(reg, &[val as u16]);
    }

    fn write2(&mut self, reg: u16, val: u32) {
        self.write(reg, &[(val >> 16) as u16, val as u16]);
    }

    fn cmd(&mut self, c: u16) {
        self.write(reg::CMD, &[c]);
    }

    /// Long FDRI write, followed by the automatic CRC.
    fn fdri(&mut self, words: &[u16]) {
        self.word(0x5000 | reg::FDRI << 5);
        self.out.extend_from_slice(&(words.len() as u32).to_be_bytes());
        for &w in words {
            self.crc.update(reg::FDRI, w);
            self.word(w);
        }
        self.out.extend_from_slice(&self.crc.0.to_be_bytes());
    }
}

fn far(addr: FrameAddr) -> u32 {
    if addr.typ == 1 {
        addr.minor << 14 | addr.major << 16 | addr.region << 24 | addr.typ << 28
    } else {
        addr.minor | addr.major << 16 | addr.region << 24 | addr.typ << 28
    }
}

/// A frame's bits as 16-bit words, in the order the parser expects: word i
/// holds frame bits [len - 16(i+1), len - 16i), LSB first.
fn frame_words(bits: impl Fn(usize) -> bool, len: usize) -> Vec<u16> {
    (0..len / 16)
        .map(|i| {
            let base = len - (i + 1) * 16;
            (0..16).fold(0u16, |w, j| w | (bits(base + j) as u16) << j)
        })
        .collect()
}

pub fn write_bitstream(bs: &Bitstream) -> Vec<u8> {
    let r = |reg: Reg| *bs.regs.get(&reg).unwrap_or_else(|| panic!("register {reg:?} missing"));
    let mut w = Writer { out: vec![], crc: Crc(0) };
    for _ in 0..8 {
        w.word(0xffff);
    }
    w.word(0xaa99);
    w.word(0x5566);
    w.cmd(cmd::RCRC);
    w.nops(1);
    w.write1(reg::FLR, (bs.iob.len() / 16) as u32);
    w.write1(reg::COR1, r(Reg::Cor1));
    w.write1(reg::COR2, r(Reg::Cor2));
    w.write2(reg::IDCODE, r(Reg::Idcode));
    // MASK selects the CTL0 bits being written. ISE's values (checked by the
    // roundtrip test): 0x00cf here, 0x00ff in the startup sequence.
    w.write1(reg::MASK, 0x00cf);
    w.write1(reg::CTL0, r(Reg::Ctl0));
    w.nops(17);
    w.write1(reg::CCLK_FREQ, r(Reg::CclkFrequency));
    w.write1(reg::PWRDN, r(Reg::Powerdown));
    w.write1(reg::EYE_MASK, r(Reg::EyeMask));
    w.write1(reg::HC_OPT, r(Reg::HcOpt));
    w.write1(reg::TIMER, r(Reg::Timer));
    w.write1(reg::PU_GWE, r(Reg::PuGwe));
    w.write1(reg::PU_GTS, r(Reg::PuGts));
    w.write1(reg::MODE, r(Reg::Mode));
    w.write1(reg::GENERAL1, r(Reg::General1));
    w.write1(reg::GENERAL2, r(Reg::General2));
    w.write1(reg::GENERAL3, r(Reg::General3));
    w.write1(reg::GENERAL4, r(Reg::General4));
    w.write1(reg::GENERAL5, r(Reg::General5));
    w.write1(reg::SEU_OPT, r(Reg::SeuOpt));
    w.write2(reg::RBCRC_SW, r(Reg::RbCrcSw));
    match bs.regs.get(&Reg::Testmode) {
        Some(&v) => w.write1(reg::TESTMODE, v),
        None => w.nops(2),
    }

    // Frame data: one FDRI from frame 0. Writes go through a one-frame
    // pipeline: each frame is committed when the next one is written. Two
    // padding frames follow the last frame of each row (region), then the
    // BRAM frames, then the IOB frame plus one padding word.
    let main_words = FRAME_LEN / 16;
    let mut data: Vec<u16> = vec![];
    // Padding frames are all ones in ISE output.
    let pad = vec![0xffffu16; main_words];
    for (fi, addr) in bs.frame_info.iter().enumerate() {
        data.extend(frame_words(|b| bs.frame_data[fi * FRAME_LEN + b], FRAME_LEN));
        let last_in_region = fi + 1 == bs.frame_info.len() || bs.frame_info[fi + 1].region != addr.region;
        if last_in_region {
            data.extend(&pad);
            data.extend(&pad);
        }
    }
    for fi in 0..bs.bram_frame_info.len() {
        data.extend(frame_words(|b| bs.bram_data[fi * BRAM_FRAME_LEN + b], BRAM_FRAME_LEN));
    }
    data.extend(frame_words(|b| bs.iob[b], bs.iob.len()));
    data.push(0);
    w.write2(reg::FAR, far(bs.frame_info[0]));
    w.cmd(cmd::WCFG);
    w.fdri(&data);
    w.nops(24);

    assert!(bs.frame_fixups.is_empty(), "frame fixups are not supported");

    w.cmd(cmd::GRESTORE);
    w.cmd(cmd::DGHIGH);
    w.nops(4);
    w.cmd(cmd::GRESTORE);
    w.cmd(cmd::START);
    w.write1(reg::MASK, 0x00ff);
    w.write1(reg::CTL0, r(Reg::Ctl0));
    let crc = w.crc.0;
    w.write2(reg::CRC, crc);
    w.cmd(cmd::DESYNC);
    w.nops(14);
    w.out
}
