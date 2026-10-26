use std::net::Ipv4Addr;

use serde::{Deserialize, Serialize};

pub const SOURCE_SUBNET: &str = "10.202.1.0/24";

/// The second source identity is carried *inside* WireGuard, unlike an skb mark.
/// It is authorized only for router owners on a WARP-prepared Gofro server.
pub fn source_address(address: Ipv4Addr) -> Option<Ipv4Addr> {
    let [a, b, c, host] = address.octets();
    (a == 10 && b == 202 && c == 0 && (2..=254).contains(&host))
        .then_some(Ipv4Addr::new(10, 202, 1, host))
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct WarpStatus {
    pub installed: bool,
    pub enabled: bool,
    pub connected: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_identities_are_one_to_one_and_never_include_the_gateway() {
        for host in 2..=254 {
            assert_eq!(
                source_address(Ipv4Addr::new(10, 202, 0, host)),
                Some(Ipv4Addr::new(10, 202, 1, host))
            );
        }
        for value in [
            "10.202.0.0",
            "10.202.0.1",
            "10.202.0.255",
            "10.202.1.2",
            "192.168.1.2",
        ] {
            assert_eq!(source_address(value.parse().unwrap()), None);
        }
    }
}
