use std::{
    net::UdpSocket,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};

pub(crate) const BUFFER_SIZE: usize = 4096;
const HEADER_SIZE: usize = 10;
const MAX_PADDING: usize = 31;
const SEED_STEP: u64 = 0x9e37_79b9_7f4a_7c15;
static PACKET_COUNTER: AtomicU64 = AtomicU64::new(SEED_STEP);

pub(crate) fn send_encoded(socket: &UdpSocket, plain: &[u8]) -> Result<()> {
    let mut encoded = [0_u8; BUFFER_SIZE];
    let packet = encode(plain, &mut encoded)?;
    if socket.send(packet).context("UDP send failed")? != packet.len() {
        bail!("partial UDP send");
    }
    Ok(())
}

pub(crate) fn encode<'a>(plain: &[u8], output: &'a mut [u8]) -> Result<&'a [u8]> {
    let length = u16::try_from(plain.len()).context("WireGuard datagram is too large")?;
    let seed = packet_seed();
    let mut stream = seed ^ 0xa076_1d64_78bd_642f;
    let padding = usize::from(next_byte(&mut stream) & MAX_PADDING as u8);
    let size = HEADER_SIZE + plain.len() + padding;
    if output.len() < size {
        bail!("relay output buffer is too small");
    }

    output[..8].copy_from_slice(&seed.to_le_bytes());
    for (target, source) in output[8..10].iter_mut().zip(length.to_le_bytes()) {
        *target = source ^ next_byte(&mut stream);
    }
    for (target, source) in output[HEADER_SIZE..HEADER_SIZE + plain.len()]
        .iter_mut()
        .zip(plain)
    {
        *target = *source ^ next_byte(&mut stream);
    }
    for byte in &mut output[HEADER_SIZE + plain.len()..size] {
        *byte = next_byte(&mut stream);
    }
    Ok(&output[..size])
}

pub(crate) fn decode<'a>(packet: &[u8], output: &'a mut [u8]) -> Option<&'a [u8]> {
    if packet.len() < HEADER_SIZE {
        return None;
    }
    let seed = u64::from_le_bytes(packet[..8].try_into().ok()?);
    let mut stream = seed ^ 0xa076_1d64_78bd_642f;
    let padding = usize::from(next_byte(&mut stream) & MAX_PADDING as u8);
    let length = usize::from(u16::from_le_bytes([
        packet[8] ^ next_byte(&mut stream),
        packet[9] ^ next_byte(&mut stream),
    ]));
    if length > output.len() || HEADER_SIZE + length + padding != packet.len() {
        return None;
    }
    for (target, source) in output[..length]
        .iter_mut()
        .zip(&packet[HEADER_SIZE..HEADER_SIZE + length])
    {
        *target = *source ^ next_byte(&mut stream);
    }
    Some(&output[..length])
}

fn packet_seed() -> u64 {
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    PACKET_COUNTER.fetch_add(SEED_STEP, Ordering::Relaxed) ^ time
}

fn next_byte(state: &mut u64) -> u8 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    (*state >> 56) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_round_trip_and_rejects_truncation() {
        let plain = b"wireguard datagram";
        let mut encoded = [0_u8; 128];
        let packet = encode(plain, &mut encoded).unwrap();
        assert_ne!(&packet[HEADER_SIZE..HEADER_SIZE + plain.len()], plain);

        let mut decoded = [0_u8; 128];
        assert_eq!(decode(packet, &mut decoded), Some(plain.as_slice()));
        assert!(decode(&packet[..packet.len() - 1], &mut decoded).is_none());
    }
}
