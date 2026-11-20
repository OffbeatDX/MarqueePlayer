//! A fast 128-bit hash of bytes, for telling whether data changed (a file's contents, an
//! embedded picture, a tag item's binary data). Not cryptographic; stable across builds and
//! Rust versions (it is written out in the edit journal), unlike `std`'s hasher.

/// Two independent 64-bit lanes over 8-byte words (multiply, xor-shift), the length mixed
/// in last.
#[derive(Clone, Debug)]
pub struct Hasher {
    a: u64,
    b: u64,
    len: u64,
    tail: [u8; 8],
    tail_len: usize,
}

const K1: u64 = 0x9E37_79B9_7F4A_7C15;
const K2: u64 = 0xC2B2_AE3D_27D4_EB4F;
const K3: u64 = 0x1656_67B1_9E37_79F9;

fn mix(mut x: u64) -> u64 {
    x ^= x >> 33;
    x = x.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    x ^= x >> 33;
    x = x.wrapping_mul(0xC4CE_B9FE_1A85_EC53);
    x ^ (x >> 33)
}

impl Default for Hasher {
    fn default() -> Self {
        Self { a: K1, b: K2, len: 0, tail: [0; 8], tail_len: 0 }
    }
}

impl Hasher {
    pub fn new() -> Self {
        Self::default()
    }

    #[inline]
    fn word(&mut self, w: u64) {
        self.a = (self.a ^ w.wrapping_mul(K2)).rotate_left(31).wrapping_mul(K1);
        self.b = (self.b ^ w.wrapping_mul(K3)).rotate_left(27).wrapping_mul(K2).wrapping_add(self.a);
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.len += data.len() as u64;
        if self.tail_len > 0 {
            let take = (8 - self.tail_len).min(data.len());
            self.tail[self.tail_len..self.tail_len + take].copy_from_slice(&data[..take]);
            self.tail_len += take;
            data = &data[take..];
            if self.tail_len < 8 {
                return;
            }
            let w = u64::from_le_bytes(self.tail);
            self.word(w);
            self.tail_len = 0;
        }
        let (chunks, rest) = data.as_chunks::<8>();
        for c in chunks {
            self.word(u64::from_le_bytes(*c));
        }
        self.tail[..rest.len()].copy_from_slice(rest);
        self.tail_len = rest.len();
    }

    /// A number into the hash (a length, a position), as 8 bytes.
    pub fn number(&mut self, n: u64) {
        self.update(&n.to_le_bytes());
    }

    pub fn finish128(&self) -> u128 {
        let mut h = self.clone();
        let mut last = [0u8; 8];
        last[..h.tail_len].copy_from_slice(&h.tail[..h.tail_len]);
        let w = u64::from_le_bytes(last) ^ ((h.tail_len as u64) << 56);
        h.word(w);
        let a = mix(h.a ^ h.len);
        let b = mix(h.b ^ h.len.rotate_left(17) ^ a);
        (u128::from(a) << 64) | u128::from(b)
    }

    pub fn finish(&self) -> u64 {
        let v = self.finish128();
        (v >> 64) as u64 ^ v as u64
    }
}

/// The 64-bit hash of some bytes.
pub fn of(data: &[u8]) -> u64 {
    let mut h = Hasher::new();
    h.update(data);
    h.finish()
}

/// The 128-bit hash of a whole file, as 32 hex digits, read in large blocks.
pub fn file_hex(path: &std::path::Path) -> std::io::Result<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut h = Hasher::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(format!("{:032x}", h.finish128()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PINNED: &str = "663f576fcec9e9d6";

    #[test]
    fn split_updates_hash_the_same_and_bytes_matter() {
        let data: Vec<u8> = (0..1000u32).map(|i| (i * 7 % 251) as u8).collect();
        let whole = of(&data);
        let mut h = Hasher::new();
        for c in data.chunks(13) {
            h.update(c);
        }
        assert_eq!(h.finish(), whole);
        let mut other = data.clone();
        other[500] ^= 1;
        assert_ne!(of(&other), whole);
        assert_ne!(of(&data[..999]), whole, "length matters");
        assert_ne!(of(&[0]), of(&[0, 0]));
        assert_ne!(of(b""), of(&[0]));
        // Stable: written in journals, so it must not change between builds.
        assert_eq!(format!("{:016x}", of(b"marquee")), PINNED);
    }
}
