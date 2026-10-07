//! Shared IPv4 WireGuard/relay packet budget used by both VPN endpoints.
#![forbid(unsafe_code)]

pub const IPV4_UDP_OVERHEAD: usize = 20 + 8;
pub const WIREGUARD_DATA_OVERHEAD: usize = 16 + 16;
pub const RELAY_HEADER_SIZE: usize = 10;
pub const RELAY_MAX_PADDING: usize = 31;
pub const MIN_UPLINK_MTU: usize = 1480;
pub const MIN_TUNNEL_MTU: u16 = 1280;

/// WireGuard caps its alignment padding at the tunnel MTU. Do not round this
/// value up: a 1350-byte QUIC datagram needs 1378 inner bytes, and the worst-case
/// encoded IPv4 packet must still fit a 1480-byte PPPoE uplink without fragments.
pub const TUNNEL_MTU: u16 = (MIN_UPLINK_MTU
    - IPV4_UDP_OVERHEAD
    - WIREGUARD_DATA_OVERHEAD
    - RELAY_HEADER_SIZE
    - RELAY_MAX_PADDING) as u16;

/// Conservative TCP segmentation also works with explicitly reduced native MTUs.
pub const TCP_MSS: u16 = MIN_TUNNEL_MTU - 20 - 20;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quic_initial_fits_the_pppoe_budget_at_both_endpoints() {
        for datagram in [1200, 1350, 1351] {
            assert!(datagram + IPV4_UDP_OVERHEAD <= usize::from(TUNNEL_MTU));
        }
        assert_eq!(
            usize::from(TUNNEL_MTU)
                + WIREGUARD_DATA_OVERHEAD
                + RELAY_HEADER_SIZE
                + RELAY_MAX_PADDING
                + IPV4_UDP_OVERHEAD,
            MIN_UPLINK_MTU
        );
        assert_eq!(TUNNEL_MTU, 1379);
    }
}
