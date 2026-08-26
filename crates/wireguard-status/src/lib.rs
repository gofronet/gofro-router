#![forbid(unsafe_code)]

use std::{
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use serde::Serialize;

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct PeerStatus {
    pub public_key: String,
    pub endpoint: Option<String>,
    pub allowed_ips: Vec<String>,
    pub latest_handshake: Option<u64>,
    pub handshake_age_seconds: Option<u64>,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub persistent_keepalive: Option<u16>,
}

fn run(command: &mut Command) -> Result<String> {
    let description = format!("{command:?}");
    let output = command
        .output()
        .with_context(|| format!("failed to run {description}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        bail!("{description} failed: {stderr}");
    }

    String::from_utf8(output.stdout).context("command returned non-UTF-8 output")
}

pub fn wireguard_peers(interface: &str) -> Result<Vec<PeerStatus>> {
    let dump = run(Command::new("wg").args(["show", interface, "dump"]))?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before Unix epoch")?
        .as_secs();
    parse_wireguard_dump(&dump, now)
}

fn parse_wireguard_dump(dump: &str, now: u64) -> Result<Vec<PeerStatus>> {
    let mut lines = dump.lines();
    let Some(interface_line) = lines.next() else {
        return Ok(Vec::new());
    };

    if interface_line.split('\t').count() < 4 {
        bail!("invalid WireGuard interface dump");
    }

    lines
        .map(|line| {
            let fields: Vec<_> = line.split('\t').collect();
            if fields.len() != 8 {
                bail!("invalid WireGuard peer dump");
            }

            let latest_handshake = parse_u64(fields[4], "latest handshake")?;
            let keepalive = if fields[7] == "off" {
                0
            } else {
                fields[7]
                    .parse::<u16>()
                    .context("invalid persistent keepalive")?
            };

            Ok(PeerStatus {
                public_key: fields[0].to_owned(),
                endpoint: (fields[2] != "(none)").then(|| fields[2].to_owned()),
                allowed_ips: fields[3].split(',').map(str::to_owned).collect(),
                latest_handshake: (latest_handshake != 0).then_some(latest_handshake),
                handshake_age_seconds: (latest_handshake != 0)
                    .then(|| now.saturating_sub(latest_handshake)),
                rx_bytes: parse_u64(fields[5], "received bytes")?,
                tx_bytes: parse_u64(fields[6], "transmitted bytes")?,
                persistent_keepalive: (keepalive != 0).then_some(keepalive),
            })
        })
        .collect()
}

fn parse_u64(value: &str, field: &str) -> Result<u64> {
    value.parse().with_context(|| format!("invalid {field}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_peer_without_exposing_interface_key() {
        let dump = concat!(
            "PRIVATE\tPUBLIC\t51820\toff\n",
            "PEER\t(none)\t203.0.113.10:51820\t0.0.0.0/0\t100\t123\t456\t25\n"
        );

        assert_eq!(
            parse_wireguard_dump(dump, 130).unwrap(),
            vec![PeerStatus {
                public_key: "PEER".into(),
                endpoint: Some("203.0.113.10:51820".into()),
                allowed_ips: vec!["0.0.0.0/0".into()],
                latest_handshake: Some(100),
                handshake_age_seconds: Some(30),
                rx_bytes: 123,
                tx_bytes: 456,
                persistent_keepalive: Some(25),
            }]
        );
    }

    #[test]
    fn accepts_interface_without_peers() {
        assert!(
            parse_wireguard_dump("PRIVATE\tPUBLIC\t51820\toff\n", 0)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn accepts_disabled_keepalive() {
        let dump = concat!(
            "PRIVATE\tPUBLIC\t51820\toff\n",
            "PEER\t(none)\t(none)\t10.0.0.2/32\t0\t0\t0\toff\n"
        );

        let peer = parse_wireguard_dump(dump, 0).unwrap().remove(0);
        assert_eq!(peer.persistent_keepalive, None);
    }
}
