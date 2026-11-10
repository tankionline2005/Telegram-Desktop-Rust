//! Creation of a permanent auth key (https://core.telegram.org/mtproto/auth_key).

use std::time::{Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use num_bigint::BigUint;
use tdr_crypto::{
    AuthKey, aes_ige_decrypt, aes_ige_encrypt, dh, fill_random, random_bytes, rsa, sha1,
};
use tdr_tl::mtproto::{enums, functions, types};
use tdr_tl::{Deserializable, Reader as TlReader, RemoteCall, Serializable};

use crate::transport::{Reader, Writer};
use crate::{Error, MsgIdGen};

/// Result of a successful key exchange.
#[derive(Debug, Clone)]
pub struct CreatedKey {
    pub auth_key: AuthKey,
    /// `server_time - local_time`, seconds.
    pub time_offset: i32,
    pub first_salt: i64,
}

fn security(msg: &'static str) -> Error {
    Error::Security(msg)
}

/// Sends an unencrypted request and reads the unencrypted answer.
async fn plain_call<R: RemoteCall>(
    r: &mut Reader,
    w: &mut Writer,
    ids: &mut MsgIdGen,
    request: &R,
) -> Result<R::Return, Error> {
    let started = Instant::now();
    let body = request.to_bytes();
    let mut msg = Vec::with_capacity(20 + body.len());
    msg.extend_from_slice(&0u64.to_le_bytes()); // auth_key_id = 0
    msg.extend_from_slice(&ids.next_id().to_le_bytes());
    msg.extend_from_slice(&(body.len() as u32).to_le_bytes());
    msg.extend_from_slice(&body);
    w.send(&msg).await?;

    let frame = r.recv().await?;
    if frame.len() < 20 || frame[..8] != [0; 8] {
        return Err(security("expected an unencrypted message"));
    }
    let len = u32::from_le_bytes(frame[16..20].try_into().unwrap()) as usize;
    if frame.len() < 20 + len {
        return Err(security("unencrypted message is truncated"));
    }
    let body = frame.slice(20..20 + len);
    tracing::debug!(elapsed = ?started.elapsed(), "handshake round trip");
    Ok(R::Return::deserialize(&mut TlReader::new(&body))?)
}

fn tmp_aes(new_nonce: &[u8; 32], server_nonce: &[u8; 16]) -> ([u8; 32], [u8; 32]) {
    let ns = sha1(&[new_nonce, server_nonce]);
    let sn = sha1(&[server_nonce, new_nonce]);
    let nn = sha1(&[new_nonce, new_nonce]);
    let mut key = [0u8; 32];
    key[..20].copy_from_slice(&ns);
    key[20..].copy_from_slice(&sn[..12]);
    let mut iv = [0u8; 32];
    iv[..8].copy_from_slice(&sn[12..]);
    iv[8..28].copy_from_slice(&nn);
    iv[28..].copy_from_slice(&new_nonce[..4]);
    (key, iv)
}

fn be_bytes(x: u64) -> Bytes {
    let b = x.to_be_bytes();
    let first = b.iter().position(|&v| v != 0).unwrap_or(7);
    Bytes::copy_from_slice(&b[first..])
}

fn to_256(x: &BigUint) -> [u8; 256] {
    let b = x.to_bytes_be();
    let mut out = [0u8; 256];
    out[256 - b.len()..].copy_from_slice(&b);
    out
}

/// Runs the DH key exchange on a fresh connection.
///
/// `dc_id` is the DC number as sent in `p_q_inner_data_dc`: negative for media-only DCs,
/// +10000 in the test environment.
pub async fn create_auth_key(
    r: &mut Reader,
    w: &mut Writer,
    dc_id: i32,
    test: bool,
) -> Result<CreatedKey, Error> {
    let mut ids = MsgIdGen::new(0);

    // 1. req_pq_multi
    let nonce: [u8; 16] = random_bytes();
    let enums::ResPQ::ResPQ(res) =
        plain_call(r, w, &mut ids, &functions::ReqPqMulti { nonce }).await?;
    if res.nonce != nonce {
        return Err(security("resPQ nonce mismatch"));
    }
    let server_nonce = res.server_nonce;
    let keys = rsa::builtin_keys(test);
    let key = keys
        .iter()
        .find(|k| {
            res.server_public_key_fingerprints
                .contains(&k.fingerprint())
        })
        .ok_or(security("server offered no known RSA key"))?;

    // 2. pq = p * q
    if res.pq.len() > 8 {
        return Err(security("pq is too large"));
    }
    let mut pq_be = [0u8; 8];
    pq_be[8 - res.pq.len()..].copy_from_slice(&res.pq);
    let (p, q) = tdr_crypto::factorize(u64::from_be_bytes(pq_be))
        .ok_or(security("could not factorize pq"))?;

    // 3. req_DH_params with RSA_PAD-encrypted p_q_inner_data_dc
    let new_nonce: [u8; 32] = random_bytes();
    let inner = enums::PQInnerData::Dc(Box::new(types::PQInnerDataDc {
        pq: res.pq.clone(),
        p: be_bytes(p),
        q: be_bytes(q),
        nonce,
        server_nonce,
        new_nonce,
        dc: dc_id,
    }));
    let encrypted = key
        .encrypt_pad(&inner.to_bytes())
        .ok_or(security("p_q_inner_data is too long"))?;
    let dh_params = plain_call(
        r,
        w,
        &mut ids,
        &functions::ReqDHParams {
            nonce,
            server_nonce,
            p: be_bytes(p),
            q: be_bytes(q),
            public_key_fingerprint: key.fingerprint(),
            encrypted_data: Bytes::copy_from_slice(&encrypted),
        },
    )
    .await?;
    let ok = match dh_params {
        enums::ServerDHParams::Ok(ok) => ok,
        enums::ServerDHParams::Fail(_) => return Err(security("server_DH_params_fail")),
    };
    if ok.nonce != nonce || ok.server_nonce != server_nonce {
        return Err(security("server_DH_params nonce mismatch"));
    }

    // 4. Decrypt server_DH_inner_data and verify its hash.
    let (tmp_key, tmp_iv) = tmp_aes(&new_nonce, &server_nonce);
    if ok.encrypted_answer.len() % 16 != 0 || ok.encrypted_answer.len() < 32 {
        return Err(security("bad encrypted_answer length"));
    }
    let mut answer = ok.encrypted_answer.to_vec();
    aes_ige_decrypt(&mut answer, &tmp_key, &tmp_iv);
    let answer = Bytes::from(answer);
    let body = answer.slice(20..);
    let mut reader = TlReader::new(&body);
    let enums::ServerDHInnerData::ServerDHInnerData(dh_inner) =
        enums::ServerDHInnerData::deserialize(&mut reader)?;
    if sha1(&[&body[..reader.position()]]) != answer[..20] {
        return Err(security("server_DH_inner_data hash mismatch"));
    }
    if dh_inner.nonce != nonce || dh_inner.server_nonce != server_nonce {
        return Err(security("server_DH_inner_data nonce mismatch"));
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let time_offset = (dh_inner.server_time as i64 - now) as i32;

    // 5. Check DH parameters (security guidelines).
    let g = u32::try_from(dh_inner.g).map_err(|_| security("bad g"))?;
    if !dh::is_good_prime(&dh_inner.dh_prime, g) {
        return Err(security("bad dh_prime or g"));
    }
    let prime = BigUint::from_bytes_be(&dh_inner.dh_prime);
    let g_a = BigUint::from_bytes_be(&dh_inner.g_a);
    if !dh::is_good_modexp(&g_a, &prime) {
        return Err(security("bad g_a"));
    }

    // 6. Our half and the shared key; retry with a new `b` if the server asks to.
    let mut retry_id = 0i64;
    for _ in 0..5 {
        let b = BigUint::from_bytes_be(&random_bytes::<256>());
        let g_b = BigUint::from(g).modpow(&b, &prime);
        if !dh::is_good_modexp(&g_b, &prime) {
            continue;
        }
        let auth_key = AuthKey::from_bytes(to_256(&g_a.modpow(&b, &prime)));

        let client_inner =
            enums::ClientDHInnerData::ClientDHInnerData(Box::new(types::ClientDHInnerData {
                nonce,
                server_nonce,
                retry_id,
                g_b: Bytes::from(g_b.to_bytes_be()),
            }))
            .to_bytes();
        let mut data = Vec::with_capacity(20 + client_inner.len() + 16);
        data.extend_from_slice(&sha1(&[&client_inner]));
        data.extend_from_slice(&client_inner);
        let pad = (16 - data.len() % 16) % 16;
        let start = data.len();
        data.resize(start + pad, 0);
        fill_random(&mut data[start..]);
        aes_ige_encrypt(&mut data, &tmp_key, &tmp_iv);

        let answer = plain_call(
            r,
            w,
            &mut ids,
            &functions::SetClientDHParams {
                nonce,
                server_nonce,
                encrypted_data: Bytes::from(data),
            },
        )
        .await?;

        let aux = auth_key.aux_hash();
        let expected =
            |n: u8| -> [u8; 16] { sha1(&[&new_nonce, &[n], &aux])[4..20].try_into().unwrap() };
        match answer {
            enums::SetClientDHParamsAnswer::DhGenOk(x) => {
                if x.nonce != nonce || x.server_nonce != server_nonce {
                    return Err(security("dh_gen_ok nonce mismatch"));
                }
                if x.new_nonce_hash1 != expected(1) {
                    return Err(security("new_nonce_hash1 mismatch"));
                }
                let mut salt = [0u8; 8];
                for i in 0..8 {
                    salt[i] = new_nonce[i] ^ server_nonce[i];
                }
                return Ok(CreatedKey {
                    auth_key,
                    time_offset,
                    first_salt: i64::from_le_bytes(salt),
                });
            }
            enums::SetClientDHParamsAnswer::DhGenRetry(x) => {
                if x.new_nonce_hash2 != expected(2) {
                    return Err(security("new_nonce_hash2 mismatch"));
                }
                retry_id = i64::from_le_bytes(aux);
            }
            enums::SetClientDHParamsAnswer::DhGenFail(_) => {
                return Err(security("dh_gen_fail"));
            }
        }
    }
    Err(security("too many DH retries"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn be_bytes_strips_leading_zeros() {
        assert_eq!(&be_bytes(0x494C553B)[..], &[0x49, 0x4C, 0x55, 0x3B]);
    }
}
