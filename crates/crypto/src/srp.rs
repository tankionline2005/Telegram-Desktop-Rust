//! SRP-2048 check of the cloud password (two-step verification),
//! https://core.telegram.org/api/srp.

use num_bigint::BigUint;
use sha2::Sha512;

use crate::{dh, random_bytes, sha256};

/// Parameters of `passwordKdfAlgoSHA256SHA256PBKDF2HMACSHA512iter100000SHA256ModPow`
/// together with the server's `srp_B`.
pub struct SrpParams<'a> {
    pub salt1: &'a [u8],
    pub salt2: &'a [u8],
    pub g: u32,
    pub p: &'a [u8],
    pub srp_b: &'a [u8],
}

/// What goes into `inputCheckPasswordSRP` (with the `srp_id` from `account.password`).
pub struct SrpAnswer {
    pub a: Vec<u8>,
    pub m1: Vec<u8>,
}

fn pad256(x: &BigUint) -> [u8; 256] {
    let b = x.to_bytes_be();
    let mut out = [0u8; 256];
    out[256 - b.len()..].copy_from_slice(&b);
    out
}

fn sh(data: &[u8], salt: &[u8]) -> [u8; 32] {
    sha256(&[salt, data, salt])
}

/// The slow part (PBKDF2 with 100 000 iterations): `x` depends only on the password and
/// salts, so callers may compute it in a background thread.
pub fn password_hash(password: &[u8], salt1: &[u8], salt2: &[u8]) -> [u8; 32] {
    let ph1 = sh(&sh(password, salt1), salt2);
    let mut pbkdf = [0u8; 64];
    pbkdf2::pbkdf2_hmac::<Sha512>(&ph1, salt1, 100_000, &mut pbkdf);
    sh(&pbkdf, salt2)
}

/// Computes `A` and `M1`. Returns `None` if the server parameters fail the checks.
pub fn compute(params: &SrpParams<'_>, password: &[u8]) -> Option<SrpAnswer> {
    compute_with_hash(params, &password_hash(password, params.salt1, params.salt2))
}

pub fn compute_with_hash(params: &SrpParams<'_>, x_hash: &[u8; 32]) -> Option<SrpAnswer> {
    if params.p.len() != 256 || !dh::is_good_prime(params.p, params.g) {
        return None;
    }
    let p = BigUint::from_bytes_be(params.p);
    let g = BigUint::from(params.g);
    let g_b = BigUint::from_bytes_be(params.srp_b);
    if g_b.bits() == 0 || g_b >= p {
        return None;
    }
    let g_padded = pad256(&g);
    let g_b_padded = pad256(&g_b);

    let x = BigUint::from_bytes_be(x_hash);
    let v = g.modpow(&x, &p);
    let k = BigUint::from_bytes_be(&sha256(&[params.p, &g_padded]));
    let kv = (k * &v) % &p;

    // Pick `a` until g_a passes the checks.
    let (a, g_a) = loop {
        let a = BigUint::from_bytes_be(&random_bytes::<256>());
        let g_a = g.modpow(&a, &p);
        if dh::is_good_modexp(&g_a, &p) {
            break (a, g_a);
        }
    };
    let g_a_padded = pad256(&g_a);
    let u = BigUint::from_bytes_be(&sha256(&[&g_a_padded, &g_b_padded]));
    if u.bits() == 0 {
        return None;
    }
    let t = ((&g_b + &p) - &kv) % &p;
    let s_a = t.modpow(&(a + u * x), &p);
    let k_a = sha256(&[&pad256(&s_a)]);

    let hp = sha256(&[params.p]);
    let hg = sha256(&[&g_padded]);
    let mut hp_xor_hg = [0u8; 32];
    for i in 0..32 {
        hp_xor_hg[i] = hp[i] ^ hg[i];
    }
    let m1 = sha256(&[
        &hp_xor_hg,
        &sha256(&[params.salt1]),
        &sha256(&[params.salt2]),
        &g_a_padded,
        &g_b_padded,
        &k_a,
    ]);
    Some(SrpAnswer {
        a: g_a_padded.to_vec(),
        m1: m1.to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs the server side of SRP and checks that it accepts our M1.
    #[test]
    fn server_accepts_our_proof() {
        let p = dh::KNOWN_PRIME;
        let (salt1, salt2) = (b"salt-one".as_slice(), b"salt-two".as_slice());
        let password = b"hunter2";
        let pbig = BigUint::from_bytes_be(&p);
        let g = BigUint::from(3u32);
        let x = BigUint::from_bytes_be(&password_hash(password, salt1, salt2));
        let v = g.modpow(&x, &pbig);
        let k = BigUint::from_bytes_be(&sha256(&[&p, &pad256(&g)]));
        let b = BigUint::from_bytes_be(&random_bytes::<256>());
        let g_b = (k * &v + g.modpow(&b, &pbig)) % &pbig;

        let params = SrpParams {
            salt1,
            salt2,
            g: 3,
            p: &p,
            srp_b: &pad256(&g_b),
        };
        let answer = compute(&params, password).unwrap();

        // Server: S = (A * v^u)^b
        let g_a = BigUint::from_bytes_be(&answer.a);
        let u = BigUint::from_bytes_be(&sha256(&[&answer.a, &pad256(&g_b)]));
        let s_b = (g_a.clone() * v.modpow(&u, &pbig)).modpow(&b, &pbig);
        let k_b = sha256(&[&pad256(&s_b)]);
        let hp = sha256(&[&p]);
        let hg = sha256(&[&pad256(&g)]);
        let x: Vec<u8> = hp.iter().zip(hg).map(|(a, b)| a ^ b).collect();
        let m1 = sha256(&[
            &x,
            &sha256(&[salt1]),
            &sha256(&[salt2]),
            &answer.a,
            &pad256(&g_b),
            &k_b,
        ]);
        assert_eq!(answer.m1, m1.to_vec());

        // A wrong password gives a different proof.
        assert_ne!(compute(&params, b"wrong").unwrap().m1, m1.to_vec());
    }
}
