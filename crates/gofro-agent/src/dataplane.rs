use std::{
    fmt::Write as _,
    io::Write as _,
    net::Ipv4Addr,
    process::{Command, Stdio},
};

use anyhow::{Context, Result, bail};

use crate::{
    model::{IpMatch, LanContext, RouteTarget},
    routing::{LAN_RANGES, RoutingPolicy},
};

pub(crate) const DIRECT_MARK: u32 = 0x10000;
pub(crate) const VPN_MARK: u32 = 0x20000;
pub(crate) const BLOCK_MARK: u32 = 0x30000;
const GOFRO_MARK_MASK: &str = "0x30000";
const KEEP_FOREIGN_MARKS: &str = "0xfffcffff";
const TABLE: &str = "gofro_routing";
const GUARD_TABLE: &str = "gofro_guard";
const FAKE_TARGET_SETS: [(RouteTarget, &str); 3] = [
    (RouteTarget::Direct, "fake_direct"),
    (RouteTarget::Vpn, "fake_vpn"),
    (RouteTarget::Block, "fake_block"),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FakeMapping {
    pub(crate) fake: Ipv4Addr,
    pub(crate) real: Ipv4Addr,
    pub(crate) target: RouteTarget,
}

pub(crate) fn apply(
    lan: &LanContext,
    dns_port: u16,
    vpn_enabled: bool,
    policy: &RoutingPolicy,
    mappings: &[FakeMapping],
) -> Result<()> {
    run_nft(&render(lan, dns_port, vpn_enabled, policy, mappings))
}

pub(crate) fn install_guard(lan: &LanContext) -> Result<()> {
    run_nft(&render_guard(lan))
}

fn render_guard(lan: &LanContext) -> String {
    format!(
        "destroy table inet {GUARD_TABLE}\n\
         add table inet {GUARD_TABLE}\n\
         add chain inet {GUARD_TABLE} gofro_guard {{ type filter hook forward priority filter; policy accept; }}\n\
         add rule inet {GUARD_TABLE} gofro_guard iifname \"{}\" oifname != \"{}\" drop\n",
        lan.device, lan.device
    )
}

pub(crate) fn clear_guard() -> Result<()> {
    run_nft(&format!("destroy table inet {GUARD_TABLE}\n"))
}

pub(crate) fn install_mappings(mappings: &[FakeMapping]) -> Result<()> {
    run_nft(&render_mappings(mappings))
}

fn render_mappings(mappings: &[FakeMapping]) -> String {
    if mappings.is_empty() {
        return String::new();
    }
    let real = mappings
        .iter()
        .map(|mapping| format!("{} : {}", mapping.fake, mapping.real))
        .collect::<Vec<_>>()
        .join(", ");
    let mut script = format!("add element inet {TABLE} fake_to_real {{ {real} }}\n");
    script.push_str(&render_target_elements(mappings, "add"));
    script
}

fn render_target_elements(mappings: &[FakeMapping], operation: &str) -> String {
    let mut script = String::new();
    for (target, set) in FAKE_TARGET_SETS {
        let keys = mappings
            .iter()
            .filter(|mapping| mapping.target == target)
            .map(|mapping| mapping.fake.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        if !keys.is_empty() {
            writeln!(
                script,
                "{operation} element inet {TABLE} {set} {{ {keys} }}"
            )
            .unwrap();
        }
    }
    script
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
    let mut script = format!("delete element inet {TABLE} fake_to_real {{ {keys} }}\n");
    script.push_str(&render_target_elements(mappings, "delete"));
    run_nft(&script)
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

fn render(
    lan: &LanContext,
    dns_port: u16,
    vpn_enabled: bool,
    policy: &RoutingPolicy,
    mappings: &[FakeMapping],
) -> String {
    let mut script = String::new();
    writeln!(script, "destroy table inet {TABLE}").unwrap();
    writeln!(script, "add table inet {TABLE}").unwrap();
    writeln!(
        script,
        "add map inet {TABLE} fake_to_real {{ type ipv4_addr : ipv4_addr; }}"
    )
    .unwrap();
    for (_, set) in FAKE_TARGET_SETS {
        writeln!(script, "add set inet {TABLE} {set} {{ type ipv4_addr; }}").unwrap();
    }
    script.push_str(&render_mappings(mappings));

    for index in policy.ip_rule_indices() {
        let rule = &policy.config().ip_rules[index];
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
        "add rule inet {TABLE} gofro_mark iifname \"{}\" ct direction reply meta mark set (meta mark & {KEEP_FOREIGN_MARKS}) | {DIRECT_MARK}",
        lan.device,
    )
    .unwrap();
    writeln!(
        script,
        "add rule inet {TABLE} gofro_mark iifname \"{}\" ct direction reply ct mark set (ct mark & {KEEP_FOREIGN_MARKS}) | {DIRECT_MARK}",
        lan.device,
    )
    .unwrap();
    writeln!(
        script,
        "add rule inet {TABLE} gofro_mark iifname \"{}\" ct direction reply return",
        lan.device
    )
    .unwrap();
    // Re-evaluate established original-direction packets after every policy change.
    writeln!(
        script,
        "add rule inet {TABLE} gofro_mark iifname \"{}\" meta mark set meta mark & {KEEP_FOREIGN_MARKS}",
        lan.device,
    )
    .unwrap();
    writeln!(
        script,
        "add rule inet {TABLE} gofro_mark iifname \"{}\" fib daddr type local meta mark set (meta mark & {KEEP_FOREIGN_MARKS}) | {DIRECT_MARK}",
        lan.device,
    )
    .unwrap();
    writeln!(
        script,
        "add rule inet {TABLE} gofro_mark iifname \"{}\" ip daddr {} meta mark set (meta mark & {KEEP_FOREIGN_MARKS}) | {DIRECT_MARK}",
        lan.device, lan.subnet,
    )
    .unwrap();
    // Sets retain policy intent; only the rule's constant mark changes when VPN is off.
    for (target, set) in FAKE_TARGET_SETS {
        let mark = target_mark(effective_target(target, vpn_enabled));
        writeln!(
            script,
            "add rule inet {TABLE} gofro_mark iifname \"{}\" meta mark & {GOFRO_MARK_MASK} == 0 ip daddr @{set} meta mark set (meta mark & {KEEP_FOREIGN_MARKS}) | {mark}",
            lan.device,
        )
        .unwrap();
    }
    writeln!(
        script,
            "add rule inet {TABLE} gofro_mark iifname \"{}\" meta mark & {GOFRO_MARK_MASK} == 0 ip daddr {{ {} }} meta mark set (meta mark & {KEEP_FOREIGN_MARKS}) | {DIRECT_MARK}",
            lan.device,
        LAN_RANGES.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
    )
    .unwrap();
    for index in policy.ip_rule_indices() {
        let rule = &policy.config().ip_rules[index];
        if !rule.enabled {
            continue;
        }
        let destination = match &rule.matcher {
            IpMatch::Cidr { value } => value.clone(),
            IpMatch::GeoIp { .. } => format!("@ip_rule_{index}"),
        };
        let mark = target_mark(effective_target(rule.target, vpn_enabled));
        writeln!(
            script,
            "add rule inet {TABLE} gofro_mark iifname \"{}\" meta mark & {GOFRO_MARK_MASK} == 0 ip daddr {destination} meta mark set (meta mark & {KEEP_FOREIGN_MARKS}) | {mark}",
            lan.device,
        )
        .unwrap();
    }
    let fallback = policy.effective_fallback();
    let default_mark = target_mark(effective_target(fallback, vpn_enabled));
    writeln!(
        script,
        "add rule inet {TABLE} gofro_mark iifname \"{}\" meta mark & {GOFRO_MARK_MASK} == 0 meta mark set (meta mark & {KEEP_FOREIGN_MARKS}) | {default_mark}",
        lan.device,
    )
    .unwrap();
    for mark in [DIRECT_MARK, VPN_MARK, BLOCK_MARK] {
        writeln!(
            script,
            "add rule inet {TABLE} gofro_mark iifname \"{}\" meta mark & {GOFRO_MARK_MASK} == {mark} ct mark set (ct mark & {KEEP_FOREIGN_MARKS}) | {mark}",
            lan.device,
        )
        .unwrap();
    }
    writeln!(
        script,
        "add rule inet {TABLE} gofro_mark iifname \"{}\" meta mark & {GOFRO_MARK_MASK} == {BLOCK_MARK} drop",
        lan.device,
    )
    .unwrap();
    writeln!(
        script,
        "add chain inet {TABLE} gofro_dnat {{ type nat hook prerouting priority dstnat; policy accept; }}"
    )
    .unwrap();
    writeln!(
        script,
        "add rule inet {TABLE} gofro_dnat iifname \"{}\" dnat ip to ip daddr map @fake_to_real",
        lan.device,
    )
    .unwrap();
    writeln!(
        script,
        "add chain inet {TABLE} gofro_dns {{ type nat hook prerouting priority -101; policy accept; }}"
    )
    .unwrap();
    writeln!(
        script,
        "add rule inet {TABLE} gofro_dns iifname \"{}\" udp dport 53 redirect to :{dns_port}",
        lan.device,
    )
    .unwrap();
    writeln!(
        script,
        "add rule inet {TABLE} gofro_dns iifname \"{}\" tcp dport 53 redirect to :{dns_port}",
        lan.device,
    )
    .unwrap();
    if vpn_enabled {
        writeln!(
            script,
            "add chain inet {TABLE} gofro_ipv6 {{ type filter hook forward priority filter; policy accept; }}"
        )
        .unwrap();
        writeln!(
            script,
            "add rule inet {TABLE} gofro_ipv6 iifname \"{}\" meta nfproto ipv6 drop",
            lan.device,
        )
        .unwrap();
    }
    script
}

pub(crate) fn effective_target(target: RouteTarget, vpn_enabled: bool) -> RouteTarget {
    match target {
        RouteTarget::Vpn if !vpn_enabled => RouteTarget::Direct,
        target => target,
    }
}

pub(crate) fn target_mark(target: RouteTarget) -> u32 {
    match target {
        RouteTarget::Direct => DIRECT_MARK,
        RouteTarget::Vpn => VPN_MARK,
        RouteTarget::Block => BLOCK_MARK,
    }
}

fn run_nft(script: &str) -> Result<()> {
    if script.is_empty() {
        return Ok(());
    }
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
mod tests;
