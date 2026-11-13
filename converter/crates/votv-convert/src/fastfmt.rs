//! Fast writers for the OBJ text. They produce exactly what `format!("{:.6}", x)` and `format!("{}", n)` produce, so the output
//! stays byte-identical to the slow path (this is what the tests below check).
//!
//! `{:.6}` rounds the *exact binary value* of the float (ties to even). Multiplying by 1e6 is only correct when the product is not
//! within 1e-3 of a rounding tie (the multiplication error is below 1e-4 for |x| < 1e6); in that rare case, and for huge or
//! non-finite values, the standard formatter decides.

use std::hash::{BuildHasherDefault, Hasher};

const NEG: u64 = 1 << 63;

/// `{:.6}` of `v` as a packed integer: bit 63 = sign (kept for `-0.000000`, as the formatter keeps it), the rest = millionths.
/// `None` when `v` is not finite or its magnitude is 1e6 or more.
pub fn fix6(v: f64) -> Option<u64> {
    if !(v.abs() < 1.0e6) {
        return None;
    }
    let neg = v.is_sign_negative();
    let x = v.abs() * 1.0e6;
    let fl = x.floor();
    let frac = x - fl;
    let mag = if (frac - 0.5).abs() < 1.0e-3 {
        // too close to a tie: let the standard formatter round the exact binary value, then read the digits back
        format!("{:.6}", v.abs()).bytes().filter(|b| b.is_ascii_digit()).fold(0u64, |a, b| a * 10 + (b - b'0') as u64)
    } else if frac > 0.5 {
        fl as u64 + 1
    } else {
        fl as u64
    };
    Some(if neg { mag | NEG } else { mag })
}

/// Append a packed value made by [`fix6`] as text.
pub fn write_fix6(out: &mut Vec<u8>, packed: u64) {
    if packed & NEG != 0 {
        out.push(b'-');
    }
    let mag = packed & !NEG;
    push_uint(out, mag / 1_000_000);
    out.push(b'.');
    let mut r = (mag % 1_000_000) as u32;
    let mut d = [b'0'; 6];
    for i in (0..6).rev() {
        d[i] = b'0' + (r % 10) as u8;
        r /= 10;
    }
    out.extend_from_slice(&d);
}

/// Append `format!("{:.6}", v)`.
pub fn push_f6(out: &mut Vec<u8>, v: f64) {
    match fix6(v) {
        Some(p) => write_fix6(out, p),
        None => {
            use std::io::Write;
            let _ = write!(out, "{:.6}", v);
        }
    }
}

/// Append a `<prefix>x y z\n` line with each component written like `{:.6}`.
pub fn push_line3(out: &mut Vec<u8>, prefix: &[u8], v: [f64; 3]) {
    out.extend_from_slice(prefix);
    push_f6(out, v[0]);
    out.push(b' ');
    push_f6(out, v[1]);
    out.push(b' ');
    push_f6(out, v[2]);
    out.push(b'\n');
}

/// Append a decimal integer.
pub fn push_uint(out: &mut Vec<u8>, mut n: u64) {
    let mut buf = [0u8; 20];
    let mut i = 20;
    loop {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    out.extend_from_slice(&buf[i..]);
}

/// Small, fast, non-cryptographic hasher (the Fx algorithm) for integer keys. The standard SipHash is several times slower for this.
#[derive(Default, Clone, Copy)]
pub struct FxHasher {
    hash: u64,
}

const SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;

impl FxHasher {
    #[inline]
    fn add(&mut self, w: u64) {
        self.hash = (self.hash.rotate_left(5) ^ w).wrapping_mul(SEED);
    }
}

impl Hasher for FxHasher {
    #[inline]
    fn finish(&self) -> u64 {
        // the multiply leaves the entropy in the high bits; the table uses the low ones too
        self.hash.rotate_left(26)
    }
    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut b = [0u8; 8];
            b[..chunk.len()].copy_from_slice(chunk);
            self.add(u64::from_le_bytes(b));
        }
    }
    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.add(i as u64);
    }
    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.add(i);
    }
    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.add(i as u64);
    }
}

pub type FxBuild = BuildHasherDefault<FxHasher>;

#[cfg(test)]
mod tests {
    use super::*;

    fn slow(v: f64) -> String {
        format!("{:.6}", v)
    }
    fn fast(v: f64) -> String {
        let mut o = Vec::new();
        push_f6(&mut o, v);
        String::from_utf8(o).unwrap()
    }

    #[test]
    fn matches_the_standard_formatter_on_edge_values() {
        let vals = [
            0.0, -0.0, 1.0, -1.0, 0.5, 0.1234565, 0.1234575, 0.0000005, 0.0000015, 0.0000025, -0.0000005, 1e-7, -1e-7, 1e-9, -1e-9,
            0.9999995, 999999.9999995, 999999.9999994, 123456.789, 0.000001, 33.333333333, 1.0 / 3.0, 2.0 / 3.0, 12.5, 100.0000005,
            5e-7, 4.9999999e-7, 1e6, -1e6, 1e12, f64::INFINITY, f64::NEG_INFINITY, f64::NAN,
        ];
        for v in vals {
            assert_eq!(fast(v), slow(v), "value {v:?}");
        }
    }

    #[test]
    fn matches_the_standard_formatter_on_many_values() {
        let mut s: u64 = 0x9E37_79B9_7F4A_7C15;
        for i in 0..400_000u32 {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let unit = ((s >> 11) as f64) / ((1u64 << 53) as f64); // [0,1)
            let v = match i % 4 {
                0 => unit,
                1 => (unit - 0.5) * 2000.0,
                2 => ((s >> 40) as f64) / 2_000_000.0, // values with ties at the 7th decimal
                _ => (unit - 0.5) * 1e-5,
            };
            assert_eq!(fast(v), slow(v), "value {v:?}");
        }
    }

    #[test]
    fn integers_match() {
        for n in [0u64, 1, 9, 10, 99, 100, 123456789, u32::MAX as u64, u64::MAX] {
            let mut o = Vec::new();
            push_uint(&mut o, n);
            assert_eq!(String::from_utf8(o).unwrap(), n.to_string());
        }
    }

    #[test]
    fn packed_keys_distinguish_exactly_what_the_text_distinguishes() {
        let a = fix6(0.1234564).unwrap();
        let b = fix6(0.1234561).unwrap();
        assert_eq!(a, b); // both print 0.123456
        assert_ne!(fix6(0.0).unwrap(), fix6(-0.0).unwrap()); // "0.000000" vs "-0.000000"
    }
}
