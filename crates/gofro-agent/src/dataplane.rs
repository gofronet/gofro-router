use std::{
    collections::BTreeSet,
    fmt::Write as _,
    io::Write as _,
    net::Ipv4Addr,
    process::{Command, Stdio},
};

use anyhow::{Context, Result, bail};

use crate::{
    model::{IpMatch, LanContext, MacAddress, PANEL_VIRTUAL_IP, PanelPorts, RouteTarget},
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
    panel_ports: PanelPorts,
    exclusions: &[MacAddress],
) -> Result<()> {
    run_nft(&render_with_exclusions(
        lan,
        dns_port,
        vpn_enabled,
        policy,
        mappings,
        panel_ports,
        exclusions,
    ))
}

pub(crate) fn install_guard(lan: &LanContext, exclusions: &[MacAddress]) -> Result<()> {
    run_nft(&render_guard_with_exclusions(lan, exclusions))
}

#[cfg(test)]
fn render_guard(lan: &LanContext) -> String {
    render_guard_with_exclusions(lan, &[])
}

fn render_guard_with_exclusions(lan: &LanContext, exclusions: &[MacAddress]) -> String {
    let condition = if exclusions.is_empty() {
        ""
    } else {
        "ether saddr != @device_exclusions "
    };
    format!(
        "destroy table inet {GUARD_TABLE}\n\
         add table inet {GUARD_TABLE}\n\
         {}\
         add chain inet {GUARD_TABLE} gofro_guard {{ type filter hook forward priority filter; policy accept; }}\n\
         add rule inet {GUARD_TABLE} gofro_guard iifname \"{}\" oifname != \"{}\" {condition}drop\n",
        render_exclusion_set(GUARD_TABLE, exclusions),
        lan.device,
        lan.device
    )
}

fn render_exclusion_set(table: &str, exclusions: &[MacAddress]) -> String {
    let mut script = format!("add set inet {table} device_exclusions {{ type ether_addr; }}\n");
    if !exclusions.is_empty() {
        let elements = exclusions
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(
            script,
            "add element inet {table} device_exclusions {{ {elements} }}"
        )
        .unwrap();
    }
    script
}

pub(crate) fn publish_exclusions(lan: &LanContext, exclusions: &[MacAddress]) -> Result<()> {
    run_nft(&render_publish_exclusions(lan, exclusions))
}

fn render_publish_exclusions(lan: &LanContext, exclusions: &[MacAddress]) -> String {
    let mut script = format!("flush set inet {TABLE} device_exclusions\n");
    if !exclusions.is_empty() {
        let elements = exclusions
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(
            script,
            "add element inet {TABLE} device_exclusions {{ {elements} }}"
        )
        .unwrap();
    }
    script.push_str(&render_guard_with_exclusions(lan, exclusions));
    script
}

fn render_startup(
    lan: &LanContext,
    exclusions: &[MacAddress],
    table_exists: bool,
    has_set: bool,
) -> Result<String> {
    if table_exists && has_set {
        return Ok(render_publish_exclusions(lan, exclusions));
    }
    anyhow::ensure!(
        !table_exists || exclusions.is_empty(),
        "legacy routing table cannot honor saved device exclusions; full routing repair required"
    );
    Ok(render_guard_with_exclusions(lan, exclusions))
}

fn has_exclusion_return(entries: &[serde_json::Value], chain: &str) -> bool {
    entries.iter().any(|entry| {
        let rule = &entry["rule"];
        rule["chain"] == chain
            && rule["expr"].as_array().is_some_and(|expressions| {
                expressions.iter().any(|expr| expr.get("return").is_some())
                    && expressions.iter().any(|expr| {
                        let condition = &expr["match"];
                        condition["left"]["payload"]["protocol"] == "ether"
                            && condition["left"]["payload"]["field"] == "saddr"
                            && condition["right"] == "@device_exclusions"
                            && matches!(condition["op"].as_str(), Some("==" | "in"))
                    })
            })
    })
}

pub(crate) fn synchronize_startup(lan: &LanContext, exclusions: &[MacAddress]) -> Result<()> {
    let result = (|| {
        let output = Command::new("nft")
            .args(["-j", "list", "tables"])
            .output()?;
        anyhow::ensure!(output.status.success(), "failed to inspect nft tables");
        let tables: serde_json::Value = serde_json::from_slice(&output.stdout)?;
        let exists = tables["nftables"]
            .as_array()
            .context("invalid nft tables")?
            .iter()
            .any(|entry| entry["table"]["family"] == "inet" && entry["table"]["name"] == TABLE);
        if !exists {
            return run_nft(&render_startup(lan, exclusions, false, false)?);
        }
        let output = Command::new("nft")
            .args(["-j", "list", "table", "inet", TABLE])
            .output()?;
        anyhow::ensure!(output.status.success(), "failed to inspect routing table");
        let table: serde_json::Value = serde_json::from_slice(&output.stdout)?;
        let entries = table["nftables"].as_array().context("invalid nft table")?;
        let has_set = entries.iter().any(|entry| {
            entry["set"]["name"] == "device_exclusions" && entry["set"]["type"] == "ether_addr"
        });
        // Clean service stop removes DNS interception; absence is already native DNS.
        let has_dns_rules = entries
            .iter()
            .any(|entry| entry["rule"]["chain"] == "gofro_dns");
        anyhow::ensure!(
            exclusions.is_empty()
                || (has_exclusion_return(entries, "gofro_mark")
                    && (!has_dns_rules || has_exclusion_return(entries, "gofro_dns"))),
            "routing table lacks device bypass rules; full routing repair required"
        );
        run_nft(&render_startup(lan, exclusions, true, has_set)?)
    })();
    if result.is_err() {
        install_guard(lan, &[])?;
    }
    result
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

#[cfg(test)]
fn render(
    lan: &LanContext,
    dns_port: u16,
    vpn_enabled: bool,
    policy: &RoutingPolicy,
    mappings: &[FakeMapping],
    panel_ports: PanelPorts,
) -> String {
    render_with_exclusions(
        lan,
        dns_port,
        vpn_enabled,
        policy,
        mappings,
        panel_ports,
        &[],
    )
}

fn render_with_exclusions(
    lan: &LanContext,
    dns_port: u16,
    vpn_enabled: bool,
    policy: &RoutingPolicy,
    mappings: &[FakeMapping],
    panel_ports: PanelPorts,
    exclusions: &[MacAddress],
) -> String {
    let port_set = |ports: &[u16]| {
        ports
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    };
    let http_ports = port_set(&[80, 8081, panel_ports.http]);
    let https_ports = port_set(&[443, 8443, panel_ports.https]);
    let allowed_ports = port_set(&[80, 443, 8081, 8443, panel_ports.http, panel_ports.https]);
    let mut script = String::new();
    writeln!(script, "destroy table inet {TABLE}").unwrap();
    writeln!(script, "add table inet {TABLE}").unwrap();
    script.push_str(&render_exclusion_set(TABLE, exclusions));
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
    // The VIP is only a LAN TCP panel endpoint, including before DNS redirect.
    for condition in [
        format!("iifname != \"{}\"", lan.device),
        "meta l4proto != tcp".to_owned(),
        format!("tcp dport != {{ {allowed_ports} }}"),
    ] {
        writeln!(
            script,
            "add rule inet {TABLE} gofro_mark ip daddr {PANEL_VIRTUAL_IP} {condition} drop"
        )
        .unwrap();
    }
    // DNS ownership is conntrack-only; all other foreign mark bits are preserved.
    for protocol in ["udp", "tcp"] {
        writeln!(script, "add rule inet {TABLE} gofro_mark iifname \"{}\" ct direction original {protocol} dport 53 ct mark set ct mark | 0x40000000", lan.device).unwrap();
    }
    writeln!(script, "add rule inet {TABLE} gofro_mark iifname \"{}\" ether saddr @device_exclusions meta mark set (meta mark & {KEEP_FOREIGN_MARKS}) | {DIRECT_MARK} ct mark set (ct mark & {KEEP_FOREIGN_MARKS}) | {DIRECT_MARK} return", lan.device).unwrap();
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
        "add rule inet {TABLE} gofro_mark iifname \"{}\" ip daddr {PANEL_VIRTUAL_IP} meta mark set (meta mark & {KEEP_FOREIGN_MARKS}) | {DIRECT_MARK}",
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
    for (ports, destination) in [
        (http_ports, panel_ports.http),
        (https_ports, panel_ports.https),
    ] {
        writeln!(
            script,
            "add rule inet {TABLE} gofro_dnat iifname \"{}\" ip daddr {PANEL_VIRTUAL_IP} tcp dport {{ {ports} }} dnat ip to {}:{destination}",
            lan.device, lan.address,
        )
        .unwrap();
    }
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
        "add rule inet {TABLE} gofro_dns iifname \"{}\" ether saddr @device_exclusions return\nadd rule inet {TABLE} gofro_dns iifname \"{}\" udp dport 53 redirect to :{dns_port}",
        lan.device,
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
            "add rule inet {TABLE} gofro_ipv6 iifname \"{}\" ether saddr != @device_exclusions meta nfproto ipv6 drop",
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
