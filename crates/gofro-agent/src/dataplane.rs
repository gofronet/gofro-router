use std::{
    fmt::Write as _,
    io::Write as _,
    net::Ipv4Addr,
    process::{Command, Stdio},
};

use anyhow::{Context, Result, bail};

use crate::{
    model::{IpMatch, RouteTarget},
    routing::RoutingPolicy,
};

pub(crate) const DIRECT_MARK: u32 = 0x10000;
pub(crate) const VPN_MARK: u32 = 0x20000;
pub(crate) const BLOCK_MARK: u32 = 0x30000;
const TABLE: &str = "gofro_routing";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FakeMapping {
    pub(crate) fake: Ipv4Addr,
    pub(crate) real: Ipv4Addr,
    pub(crate) target: RouteTarget,
}

pub(crate) fn apply(
    lan_interface: &str,
    policy: &RoutingPolicy,
    mappings: &[FakeMapping],
) -> Result<()> {
    run_nft(&render(lan_interface, policy, mappings))
}

pub(crate) fn install_mappings(mappings: &[FakeMapping]) -> Result<()> {
    if mappings.is_empty() {
        return Ok(());
    }
    let real = mappings
        .iter()
        .map(|mapping| format!("{} : {}", mapping.fake, mapping.real))
        .collect::<Vec<_>>()
        .join(", ");
    let marks = mappings
        .iter()
        .map(|mapping| format!("{} : {}", mapping.fake, target_mark(mapping.target)))
        .collect::<Vec<_>>()
        .join(", ");
    run_nft(&format!(
        "add element inet {TABLE} fake_to_real {{ {real} }}\n\
         add element inet {TABLE} fake_to_mark {{ {marks} }}\n"
    ))
}

pub(crate) fn remove_mappings(mappings: &[FakeMapping]) -> Result<()> {
    if mappings.is_empty() {
        return Ok(());
    }
    let keys = mappings
        .iter()
        .map(|mapping| mapping.fake.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    run_nft(&format!(
        "delete element inet {TABLE} fake_to_real {{ {keys} }}\n\
         delete element inet {TABLE} fake_to_mark {{ {keys} }}\n"
    ))
}

pub(crate) fn is_installed() -> bool {
    table_exists()
}

fn table_exists() -> bool {
    Command::new("nft")
        .args(["--terse", "list", "table", "inet", TABLE])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn render(lan_interface: &str, policy: &RoutingPolicy, mappings: &[FakeMapping]) -> String {
    let mut script = String::new();
    writeln!(script, "destroy table inet {TABLE}").unwrap();
    writeln!(script, "add table inet {TABLE}").unwrap();
    writeln!(
        script,
        "add map inet {TABLE} fake_to_real {{ type ipv4_addr : ipv4_addr; }}"
    )
    .unwrap();
    writeln!(
        script,
        "add map inet {TABLE} fake_to_mark {{ type ipv4_addr : mark; }}"
    )
    .unwrap();
    if !mappings.is_empty() {
        let real = mappings
            .iter()
            .map(|mapping| format!("{} : {}", mapping.fake, mapping.real))
            .collect::<Vec<_>>()
            .join(", ");
        let marks = mappings
            .iter()
            .map(|mapping| format!("{} : {}", mapping.fake, target_mark(mapping.target)))
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(script, "add element inet {TABLE} fake_to_real {{ {real} }}").unwrap();
        writeln!(
            script,
            "add element inet {TABLE} fake_to_mark {{ {marks} }}"
        )
        .unwrap();
    }

    for (index, rule) in policy.config().ip_rules.iter().enumerate() {
        if !rule.enabled {
            continue;
        }
        if let IpMatch::GeoIp { value } = &rule.matcher {
            let networks = policy.geodata().ip_networks(value);
            writeln!(
                script,
                "add set inet {TABLE} ip_rule_{index} {{ type ipv4_addr; flags interval; auto-merge; }}"
            )
            .unwrap();
            if !networks.is_empty() {
                let elements = networks
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ");
                writeln!(
                    script,
                    "add element inet {TABLE} ip_rule_{index} {{ {elements} }}"
                )
                .unwrap();
            }
        }
    }

    writeln!(
        script,
        "add chain inet {TABLE} gofro_mark {{ type filter hook prerouting priority mangle; policy accept; }}"
    )
    .unwrap();
    writeln!(
        script,
        "add rule inet {TABLE} gofro_mark iifname \"{lan_interface}\" meta mark set 0"
    )
    .unwrap();
    writeln!(
        script,
        "add rule inet {TABLE} gofro_mark iifname \"{lan_interface}\" meta mark set ip daddr map @fake_to_mark"
    )
    .unwrap();
    writeln!(
        script,
        "add rule inet {TABLE} gofro_mark iifname \"{lan_interface}\" meta mark 0 ip daddr {{ 10.0.0.0/8, 127.0.0.0/8, 169.254.0.0/16, 172.16.0.0/12, 192.168.0.0/16 }} meta mark set {DIRECT_MARK}"
    )
    .unwrap();
    for (index, rule) in policy.config().ip_rules.iter().enumerate() {
        if !rule.enabled {
            continue;
        }
        let destination = match &rule.matcher {
            IpMatch::Cidr { value } => value.clone(),
            IpMatch::GeoIp { .. } => format!("@ip_rule_{index}"),
        };
        let mark = target_mark(rule.target);
        writeln!(
            script,
            "add rule inet {TABLE} gofro_mark iifname \"{lan_interface}\" meta mark 0 ip daddr {destination} meta mark set {mark}"
        )
        .unwrap();
    }
    let default_mark = target_mark(policy.config().default_target);
    writeln!(
        script,
        "add rule inet {TABLE} gofro_mark iifname \"{lan_interface}\" meta mark 0 meta mark set {default_mark}"
    )
    .unwrap();
    writeln!(
        script,
        "add rule inet {TABLE} gofro_mark iifname \"{lan_interface}\" ct mark set meta mark"
    )
    .unwrap();
    writeln!(
        script,
        "add rule inet {TABLE} gofro_mark iifname \"{lan_interface}\" meta mark {BLOCK_MARK} drop"
    )
    .unwrap();
    writeln!(
        script,
        "add chain inet {TABLE} gofro_dnat {{ type nat hook prerouting priority dstnat; policy accept; }}"
    )
    .unwrap();
    writeln!(
        script,
        "add rule inet {TABLE} gofro_dnat iifname \"{lan_interface}\" dnat ip to ip daddr map @fake_to_real"
    )
    .unwrap();
    script
}

pub(crate) fn target_mark(target: RouteTarget) -> u32 {
    match target {
        RouteTarget::Direct => DIRECT_MARK,
        RouteTarget::Vpn => VPN_MARK,
        RouteTarget::Block => BLOCK_MARK,
    }
}

fn run_nft(script: &str) -> Result<()> {
    let mut child = Command::new("nft")
        .args(["-f", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to start nft")?;
    child
        .stdin
        .take()
        .context("failed to open nft stdin")?
        .write_all(script.as_bytes())
        .context("failed to write nft transaction")?;
    let output = child.wait_with_output().context("failed to wait for nft")?;
    if !output.status.success() {
        bail!(
            "nft transaction failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::{
        geodata::GeoData,
        model::{IpMatch, IpRule, RoutingConfig},
    };

    #[test]
    fn renders_kernel_only_split_routing() {
        let policy = RoutingPolicy::compile(
            RoutingConfig {
                domain_rules: vec![],
                ip_rules: vec![IpRule {
                    name: "Block LAN".into(),
                    enabled: true,
                    matcher: IpMatch::Cidr {
                        value: "10.0.0.0/8".into(),
                    },
                    target: RouteTarget::Block,
                }],
                default_target: RouteTarget::Vpn,
            },
            Arc::new(GeoData::default()),
        )
        .unwrap();
        let script = render(
            "wlan0",
            &policy,
            &[FakeMapping {
                fake: "198.18.0.1".parse().unwrap(),
                real: "1.1.1.1".parse().unwrap(),
                target: RouteTarget::Direct,
            }],
        );
        assert!(script.contains("198.18.0.1 : 1.1.1.1"));
        assert!(script.contains("198.18.0.1 : 65536"));
        assert!(script.starts_with("destroy table inet gofro_routing\nadd table"));
        assert!(script.contains("meta mark set 0"));
        assert!(script.contains("meta mark 0 meta mark set 131072"));
        assert!(script.contains("meta mark 196608 drop"));
        let local = script.find("127.0.0.0/8").unwrap();
        let custom = script
            .find("ip daddr 10.0.0.0/8 meta mark set 196608")
            .unwrap();
        assert!(local < custom);
        assert!(script.contains("add chain inet gofro_routing gofro_mark"));
        assert!(script.contains("add chain inet gofro_routing gofro_dnat"));
        assert!(!script.contains("gofro_routing mark"));
        assert!(script.contains("dnat ip to ip daddr map @fake_to_real"));
    }
}
