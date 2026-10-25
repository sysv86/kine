//! MSB-first bit writer/reader for ITU-T H.266 RBSP payloads.
//!
//! H.266 packs syntax elements MSB first.  `BitWriter` is used for both the
//! parameter sets (where every write is followed by `rbsp_trailing_bits()`) and
//! the slice header (where the write is followed by `byte_alignment()`).

/// MSB-first bit writer.
#[derive(Debug, Default)]
pub struct BitWriter {
    bytes: Vec<u8>,
    /// Bits not yet flushed to `bytes`, right-aligned.
    acc: u64,
    nbits: u32,
}

impl BitWriter {
    pub fn new() -> Self {
        Self {
            bytes: Vec::with_capacity(64),
            acc: 0,
            nbits: 0,
        }
    }

    pub fn with_capacity(cap: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(cap),
            acc: 0,
            nbits: 0,
        }
    }

    #[inline]
    pub fn write_bit(&mut self, bit: bool) {
        self.acc = (self.acc << 1) | u64::from(bit);
        self.nbits += 1;
        if self.nbits == 8 {
            self.bytes.push(self.acc as u8);
            self.acc = 0;
            self.nbits = 0;
        }
    }

    /// Write the `n` least-significant bits of `value`, MSB first.
    ///
    /// `n` must be in `0..=32`; bits above `n` are ignored (this matches the
    /// permissive VTM `OutputBitstream::write` contract for `n < 32`).
    pub fn write_bits(&mut self, value: u32, n: u32) {
        debug_assert!(n <= 32);
        for i in (0..n).rev() {
            self.write_bit(((value >> i) & 1) != 0);
        }
    }

    /// Unsigned Exp-Golomb `ue(v)` (H.266 9.2).
    pub fn write_ue(&mut self, value: u32) {
        let code_num = u64::from(value) + 1;
        let bits = 64 - code_num.leading_zeros(); // number of significant bits
        for _ in 0..(bits - 1) {
            self.write_bit(false);
        }
        // Write `bits` low bits of code_num (fits in <= 32 for our ranges).
        for i in (0..bits).rev() {
            self.write_bit(((code_num >> i) & 1) != 0);
        }
    }

    /// Signed Exp-Golomb `se(v)` (H.266 9.2).
    pub fn write_se(&mut self, value: i32) {
        let code_num = if value <= 0 {
            (-i64::from(value)) as u32 * 2
        } else {
            (value as u32) * 2 - 1
        };
        self.write_ue(code_num);
    }

    pub fn bits_written(&self) -> usize {
        self.bytes.len() * 8 + self.nbits as usize
    }

    pub fn is_byte_aligned(&self) -> bool {
        self.nbits == 0
    }

    /// Pad with zero bits until the next byte boundary.
    pub fn align_zero(&mut self) {
        if self.nbits != 0 {
            let pad = 8 - self.nbits;
            self.write_bits(0, pad);
        }
    }

    /// `byte_alignment()` (H.266 7.3.2.11): a single `1` bit then zeros.
    ///
    /// Bit-identical to `rbsp_trailing_bits()` but used mid-structure (between
    /// the slice header and `slice_data()`).
    pub fn byte_alignment(&mut self) {
        self.write_bit(true);
        self.align_zero();
    }

    /// `rbsp_trailing_bits()` (H.266 7.3.2.11) at the end of an RBSP.
    pub fn rbsp_trailing_bits(&mut self) {
        self.write_bit(true);
        self.align_zero();
    }

    /// Append already byte-aligned bytes.  Panics if the writer is mid-byte.
    pub fn append_bytes(&mut self, bytes: &[u8]) {
        assert!(
            self.is_byte_aligned(),
            "append_bytes requires byte alignment"
        );
        self.bytes.extend_from_slice(bytes);
    }

    /// Finish the writer.  Panics if there are pending bits (callers must end
    /// with a trailing-bits or alignment construct).
    pub fn into_bytes(self) -> Vec<u8> {
        assert!(
            self.nbits == 0,
            "BitWriter finished with {} pending bits",
            self.nbits
        );
        self.bytes
    }
}

/// MSB-first bit reader, used for the small amount of HLS parsing the muxer
/// needs (dimensions / profile / level) and for the subset decoder's headers.
#[derive(Debug, Clone)]
pub struct BitReader<'a> {
    data: &'a [u8],
    pos: usize, // bit position
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn bits_left(&self) -> usize {
        self.data.len() * 8 - self.pos
    }

    /// Absolute bit position from the start of the buffer.
    pub fn bit_pos(&self) -> usize {
        self.pos
    }

    pub fn read_bit(&mut self) -> Result<bool, &'static str> {
        if self.pos >= self.data.len() * 8 {
            return Err("bit reader exhausted");
        }
        let byte = self.data[self.pos >> 3];
        let bit = (byte >> (7 - (self.pos & 7))) & 1;
        self.pos += 1;
        Ok(bit != 0)
    }

    pub fn read_bits(&mut self, n: u32) -> Result<u32, &'static str> {
        if n > 32 {
            return Err("read_bits supports at most 32 bits");
        }
        let mut value = 0u32;
        for _ in 0..n {
            value = (value << 1) | u32::from(self.read_bit()?);
        }
        Ok(value)
    }

    pub fn read_ue(&mut self) -> Result<u32, &'static str> {
        let mut leading = 0u32;
        while !self.read_bit()? {
            leading += 1;
            if leading > 31 {
                return Err("invalid ue(v) prefix");
            }
        }
        if leading == 0 {
            return Ok(0);
        }
        let rest = self.read_bits(leading)?;
        Ok(((1u32 << leading) - 1) + rest)
    }

    pub fn read_se(&mut self) -> Result<i32, &'static str> {
        let code_num = self.read_ue()?;
        let magnitude = (code_num + 1) >> 1;
        Ok(if code_num & 1 == 0 {
            -(magnitude as i32)
        } else {
            magnitude as i32
        })
    }

    pub fn skip_bits(&mut self, n: usize) -> Result<(), &'static str> {
        if self.pos + n > self.data.len() * 8 {
            return Err("bit reader exhausted");
        }
        self.pos += n;
        Ok(())
    }

    pub fn byte_align(&mut self) {
        self.pos = (self.pos + 7) & !7;
    }

    pub fn is_byte_aligned(&self) -> bool {
        self.pos & 7 == 0
    }
}

/// Insert RBSP emulation-prevention bytes: a `0x03` before any byte `<= 0x03`
/// that follows two zero bytes (H.266 7.4.2).
pub fn rbsp_to_ebsp(rbsp: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rbsp.len() + 16);
    let mut zeros = 0u32;
    for &b in rbsp {
        if zeros >= 2 && b <= 3 {
            out.push(0x03);
            zeros = 0;
        }
        out.push(b);
        if b == 0 {
            zeros += 1;
        } else {
            zeros = 0;
        }
    }
    out
}

/// Remove emulation-prevention bytes (`0x00 0x00 0x03` -> `0x00 0x00`).
pub fn ebsp_to_rbsp(ebsp: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(ebsp.len());
    let mut i = 0usize;
    while i < ebsp.len() {
        if i + 2 < ebsp.len() && ebsp[i] == 0 && ebsp[i + 1] == 0 && ebsp[i + 2] == 3 {
            out.push(0);
            out.push(0);
            i += 3;
        } else {
            out.push(ebsp[i]);
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exp_golomb_matches_reference_encodings() {
        let cases = [
            (0u32, vec![true]),
            (1, vec![false, true, false]),
            (2, vec![false, true, true]),
        ];
        for (value, bits) in cases {
            let mut w = BitWriter::new();
            w.write_ue(value);
            assert_eq!(w.bits_written(), bits.len());
            w.align_zero();
            let bytes = w.into_bytes();
            let mut r = BitReader::new(&bytes);
            for expected in bits {
                assert_eq!(r.read_bit().unwrap(), expected);
            }
        }
    }

    #[test]
    fn signed_exp_golomb_round_trips() {
        for value in [-5i32, -1, 0, 1, 7] {
            let mut w = BitWriter::new();
            w.write_se(value);
            w.align_zero();
            let bytes = w.into_bytes();
            let mut r = BitReader::new(&bytes);
            assert_eq!(r.read_se().unwrap(), value);
        }
    }

    #[test]
    fn emulation_prevention_round_trips() {
        let rbsp = vec![0u8, 0, 0, 1, 0, 0, 2, 0, 0, 3, 9];
        let ebsp = rbsp_to_ebsp(&rbsp);
        assert_eq!(ebsp_to_rbsp(&ebsp), rbsp);
    }
}
