//! Diffie-Hellman parameter checks from the MTProto security guidelines.
//!
//! Optimization vs tdesktop: for Telegram's well-known safe prime, tdesktop re-runs two
//! 2048-bit primality tests every time `g` is 2 or 6. The prime is known to be safe, so
//! only the cheap `g`-specific residue condition is needed. Unknown primes still get the
//! full Miller–Rabin check of `p` and `(p - 1) / 2`.

use num_bigint::BigUint;

use crate::random_bytes;

/// The 2048-bit safe prime Telegram servers use (same constant as tdesktop).
pub const KNOWN_PRIME: [u8; 256] = [
    0xC7, 0x1C, 0xAE, 0xB9, 0xC6, 0xB1, 0xC9, 0x04, 0x8E, 0x6C, 0x52, 0x2F, 0x70, 0xF1, 0x3F, 0x73,
    0x98, 0x0D, 0x40, 0x23, 0x8E, 0x3E, 0x21, 0xC1, 0x49, 0x34, 0xD0, 0x37, 0x56, 0x3D, 0x93, 0x0F,
    0x48, 0x19, 0x8A, 0x0A, 0xA7, 0xC1, 0x40, 0x58, 0x22, 0x94, 0x93, 0xD2, 0x25, 0x30, 0xF4, 0xDB,
    0xFA, 0x33, 0x6F, 0x6E, 0x0A, 0xC9, 0x25, 0x13, 0x95, 0x43, 0xAE, 0xD4, 0x4C, 0xCE, 0x7C, 0x37,
    0x20, 0xFD, 0x51, 0xF6, 0x94, 0x58, 0x70, 0x5A, 0xC6, 0x8C, 0xD4, 0xFE, 0x6B, 0x6B, 0x13, 0xAB,
    0xDC, 0x97, 0x46, 0x51, 0x29, 0x69, 0x32, 0x84, 0x54, 0xF1, 0x8F, 0xAF, 0x8C, 0x59, 0x5F, 0x64,
    0x24, 0x77, 0xFE, 0x96, 0xBB, 0x2A, 0x94, 0x1D, 0x5B, 0xCD, 0x1D, 0x4A, 0xC8, 0xCC, 0x49, 0x88,
    0x07, 0x08, 0xFA, 0x9B, 0x37, 0x8E, 0x3C, 0x4F, 0x3A, 0x90, 0x60, 0xBE, 0xE6, 0x7C, 0xF9, 0xA4,
    0xA4, 0xA6, 0x95, 0x81, 0x10, 0x51, 0x90, 0x7E, 0x16, 0x27, 0x53, 0xB5, 0x6B, 0x0F, 0x6B, 0x41,
    0x0D, 0xBA, 0x74, 0xD8, 0xA8, 0x4B, 0x2A, 0x14, 0xB3, 0x14, 0x4E, 0x0E, 0xF1, 0x28, 0x47, 0x54,
    0xFD, 0x17, 0xED, 0x95, 0x0D, 0x59, 0x65, 0xB4, 0xB9, 0xDD, 0x46, 0x58, 0x2D, 0xB1, 0x17, 0x8D,
    0x16, 0x9C, 0x6B, 0xC4, 0x65, 0xB0, 0xD6, 0xFF, 0x9C, 0xA3, 0x92, 0x8F, 0xEF, 0x5B, 0x9A, 0xE4,
    0xE4, 0x18, 0xFC, 0x15, 0xE8, 0x3E, 0xBE, 0xA0, 0xF8, 0x7F, 0xA9, 0xFF, 0x5E, 0xED, 0x70, 0x05,
    0x0D, 0xED, 0x28, 0x49, 0xF4, 0x7B, 0xF9, 0x59, 0xD9, 0x56, 0x85, 0x0C, 0xE9, 0x29, 0x85, 0x1F,
    0x0D, 0x81, 0x15, 0xF6, 0x35, 0xB1, 0x05, 0xEE, 0x2E, 0x4E, 0x15, 0xD0, 0x4B, 0x24, 0x54, 0xBF,
    0x6F, 0x4F, 0xAD, 0xF0, 0x34, 0xB1, 0x04, 0x03, 0x11, 0x9C, 0xD8, 0xE3, 0xB9, 0x2F, 0xCC, 0x5B,
];

/// The residue condition that makes `g` generate the subgroup of order `(p - 1) / 2`.
fn g_condition_holds(p: &BigUint, g: u32) -> bool {
    let m = |k: u32| (p % k).to_u32_digits().first().copied().unwrap_or(0);
    match g {
        2 => m(8) == 7,
        3 => m(3) == 2,
        4 => true,
        5 => matches!(m(5), 1 | 4),
        6 => matches!(m(24), 19 | 23),
        7 => matches!(m(7), 3 | 5 | 6),
        _ => false,
    }
}

fn is_probable_prime(n: &BigUint, rounds: usize) -> bool {
    let one = BigUint::from(1u32);
    let two = BigUint::from(2u32);
    if n < &two {
        return false;
    }
    for small in [2u32, 3, 5, 7, 11, 13, 17, 19, 23, 29, 31, 37] {
        if n == &BigUint::from(small) {
            return true;
        }
        if (n % small).bits() == 0 {
            return false;
        }
    }
    let n1 = n - &one;
    let s = n1.trailing_zeros().unwrap_or(0);
    let d = &n1 >> s;
    'witness: for _ in 0..rounds {
        let a = BigUint::from_bytes_be(&random_bytes::<256>()) % (n - 3u32) + &two;
        let mut x = a.modpow(&d, n);
        if x == one || x == n1 {
            continue;
        }
        for _ in 1..s {
            x = x.modpow(&two, n);
            if x == n1 {
                continue 'witness;
            }
        }
        return false;
    }
    true
}

/// Checks `dh_prime` and `g` sent by the server.
pub fn is_good_prime(prime: &[u8], g: u32) -> bool {
    let p = BigUint::from_bytes_be(prime);
    if p.bits() != 2048 || !g_condition_holds(&p, g) {
        return false;
    }
    if prime == KNOWN_PRIME {
        return true;
    }
    is_probable_prime(&p, 32) && is_probable_prime(&((p - 1u32) >> 1), 32)
}

/// `1 < g_a < p - 1` and both `g_a` and `p - g_a` are at least 2^(2048-64).
pub fn is_good_modexp(modexp: &BigUint, prime: &BigUint) -> bool {
    const MIN_BITS: u64 = 2048 - 64;
    if modexp >= prime {
        return false;
    }
    let diff = prime - modexp;
    diff.bits() >= MIN_BITS && modexp.bits() >= MIN_BITS && modexp.bits() <= 2048
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_prime_needs_a_proper_g() {
        // Servers use g = 3. Unlike tdesktop, which accepts g ∈ {3,4,5,7} for this prime
        // unconditionally, we also check the residue condition for g (it fails for g = 5).
        assert!(is_good_prime(&KNOWN_PRIME, 3));
        assert!(is_good_prime(&KNOWN_PRIME, 4));
        let p = BigUint::from_bytes_be(&KNOWN_PRIME);
        for g in 2..=7 {
            assert_eq!(
                is_good_prime(&KNOWN_PRIME, g),
                g_condition_holds(&p, g),
                "g = {g}"
            );
        }
        assert!(!is_good_prime(&KNOWN_PRIME, 9));
    }

    #[test]
    fn known_prime_really_is_safe() {
        // Verifies the shortcut: the constant is a safe prime.
        let p = BigUint::from_bytes_be(&KNOWN_PRIME);
        assert!(is_probable_prime(&p, 8));
        assert!(is_probable_prime(&((p - 1u32) >> 1), 8));
    }

    #[test]
    fn rejects_composites() {
        let mut bad = KNOWN_PRIME;
        bad[255] = bad[255].wrapping_add(2);
        assert!(!is_good_prime(&bad, 3) || !is_probable_prime(&BigUint::from_bytes_be(&bad), 8));
        assert!(!is_probable_prime(&BigUint::from(561u32), 16)); // Carmichael number
    }
}
