//! Memory-bounded evaluation of Gofro's existing scrypt password format.
//!
//! This implements RFC 7914 with N=16384, r=8, p=1. ROMix stores every eighth
//! V block and reconstructs intervening blocks when Integerify selects them.
//! The derived key is identical to dense scrypt; checkpoints require 2 MiB
//! instead of 16 MiB, at the cost of approximately 2.75x as many BlockMix calls.
#![forbid(unsafe_code)]

use openssl::{error::ErrorStack, hash::MessageDigest, pkcs5};
use zeroize::Zeroizing;

const N: usize = 16_384;
const R: usize = 8;
const STRIDE: usize = 8;
const WORDS: usize = 32 * R;
const BYTES: usize = WORDS * 4;
pub const CHECKPOINT_BYTES: usize = N / STRIDE * BYTES;

/// Derive the existing `gofro-scrypt-v1$16384$8$1` key, including legacy salts.
pub fn scrypt(password: &[u8], salt: &[u8], output: &mut [u8]) -> Result<(), ErrorStack> {
    let mut bytes = Zeroizing::new([0_u8; BYTES]);
    pkcs5::pbkdf2_hmac(password, salt, 1, MessageDigest::sha256(), bytes.as_mut())?;
    let mut block = Zeroizing::new([0_u32; WORDS]);
    for (word, chunk) in block.iter_mut().zip(bytes.as_chunks::<4>().0) {
        *word = u32::from_le_bytes(*chunk);
    }
    let mut checkpoints = Zeroizing::new(vec![0_u32; CHECKPOINT_BYTES / 4]);
    let mut scratch = Zeroizing::new([0_u32; WORDS]);
    for index in 0..N {
        if index % STRIDE == 0 {
            let start = index / STRIDE * WORDS;
            checkpoints[start..start + WORDS].copy_from_slice(block.as_ref());
        }
        block_mix(&mut block, &mut scratch);
    }
    let mut reconstructed = Zeroizing::new([0_u32; WORDS]);
    for _ in 0..N {
        // N is a power of two below 2^32: only Integerify's low word is needed.
        let index = block[WORDS - 16] as usize & (N - 1);
        let start = index / STRIDE * WORDS;
        reconstructed.copy_from_slice(&checkpoints[start..start + WORDS]);
        for _ in 0..index % STRIDE {
            block_mix(&mut reconstructed, &mut scratch);
        }
        for (word, other) in block.iter_mut().zip(reconstructed.iter()) {
            *word ^= other;
        }
        block_mix(&mut block, &mut scratch);
    }
    for (word, chunk) in block.iter().zip(bytes.as_chunks_mut::<4>().0) {
        chunk.copy_from_slice(&word.to_le_bytes());
    }
    pkcs5::pbkdf2_hmac(password, bytes.as_ref(), 1, MessageDigest::sha256(), output)
}

// RFC 7914 sections 3–5. All indices and allocation sizes are fixed by the
// supported credential format; no password-dependent allocation takes place.
fn block_mix(block: &mut [u32; WORDS], scratch: &mut [u32; WORDS]) {
    let mut state = Zeroizing::new([0_u32; 16]);
    state.copy_from_slice(&block[WORDS - 16..]);
    for index in 0..2 * R {
        for (word, input) in state.iter_mut().zip(&block[index * 16..index * 16 + 16]) {
            *word ^= input;
        }
        salsa8(&mut state);
        let destination = (index / 2 + if index % 2 == 0 { 0 } else { R }) * 16;
        scratch[destination..destination + 16].copy_from_slice(state.as_ref());
    }
    block.copy_from_slice(scratch);
}

#[inline(always)]
fn quarter_round(state: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    state[b] ^= state[a].wrapping_add(state[d]).rotate_left(7);
    state[c] ^= state[b].wrapping_add(state[a]).rotate_left(9);
    state[d] ^= state[c].wrapping_add(state[b]).rotate_left(13);
    state[a] ^= state[d].wrapping_add(state[c]).rotate_left(18);
}

fn salsa8(block: &mut [u32; 16]) {
    let mut state = Zeroizing::new(*block);
    for _ in 0..4 {
        quarter_round(&mut state, 0, 4, 8, 12);
        quarter_round(&mut state, 5, 9, 13, 1);
        quarter_round(&mut state, 10, 14, 2, 6);
        quarter_round(&mut state, 15, 3, 7, 11);
        quarter_round(&mut state, 0, 1, 2, 3);
        quarter_round(&mut state, 5, 6, 7, 4);
        quarter_round(&mut state, 10, 11, 8, 9);
        quarter_round(&mut state, 15, 12, 13, 14);
    }
    for (word, mixed) in block.iter_mut().zip(state.iter()) {
        *word = word.wrapping_add(*mixed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc_7914_vector() {
        let expected = "7023bdcb3afd7348461c06cd81fd38ebfda8fbba904f8e3ea9b543f6545da1f2d5432955613f0fcf62d49705242a9af9e61e85dc0d651e40dfcf017b45575887";
        let mut output = [0; 64];
        scrypt(b"pleaseletmein", b"SodiumChloride", &mut output).unwrap();
        let hex = output
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        assert_eq!(hex, expected);
    }

    #[test]
    fn matches_openssl_for_binary_passwords_hmac_boundaries_and_legacy_salts() {
        for length in [0, 1, 63, 64, 65, 128] {
            let password: Vec<_> = (0..length).map(|index| (index * 37) as u8).collect();
            let salt: Vec<_> = (0..32).map(|index| (index * 13 + length) as u8).collect();
            let mut expected = [0; 32];
            let mut actual = [0; 32];
            pkcs5::scrypt(
                &password,
                &salt,
                N as u64,
                R as u64,
                1,
                64 * 1024 * 1024,
                &mut expected,
            )
            .unwrap();
            scrypt(&password, &salt, &mut actual).unwrap();
            assert_eq!(actual, expected, "password length {length}");
        }
    }
}
