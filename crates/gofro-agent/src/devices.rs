use std::{
    collections::BTreeMap,
    io::Read,
    net::IpAddr,
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Result, anyhow, ensure};
use serde::Serialize;
use serde_json::Value;

use crate::{
    AppState,
    model::{LanContext, MacAddress},
};

const MAX_BYTES: usize = 1024 * 1024;
const MAX_ROWS: usize = 4096;
const MAX_FILES: usize = 8;
const MAX_NAME: usize = 63;
const FALLBACK: &str = "/tmp/dhcp.leases";

#[derive(Serialize)]
pub(crate) struct LanDeviceInventory {
    pub(crate) devices: Vec<LanDevice>,
    pub(crate) discovery: Discovery,
}

#[derive(Serialize)]
pub(crate) struct LanDevice {
    pub(crate) mac: MacAddress,
    pub(crate) name: Option<String>,
    pub(crate) addresses: Vec<IpAddr>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Discovery {
    Complete,
    Partial,
    Unavailable,
}

// Call from a blocking worker: all discovery shares a three-second budget.
pub(crate) fn list(state: &AppState) -> Result<LanDeviceInventory> {
    let mut devices = BTreeMap::new();
    {
        let config = state
            .config
            .lock()
            .map_err(|_| anyhow!("config lock poisoned"))?;
        for &mac in &config.device_exclusions {
            devices.insert(
                mac,
                LanDevice {
                    mac,
                    name: None,
                    addresses: vec![],
                },
            );
        }
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut failed = false;
    let mut observed = false;
    let options = match command("uci", &["-q", "show", "dhcp"], deadline)
        .and_then(|text| lease_options(&text))
    {
        Ok(options) => options,
        Err(_) => {
            failed = true;
            vec![None]
        }
    };
    let mut paths = Vec::new();
    for option in options {
        let path = match option {
            Some(option) => command("uci", &["-q", "get", &option], deadline),
            None => Ok(FALLBACK.to_owned()),
        };
        match path {
            Ok(path)
                if path.trim().starts_with('/')
                    && path.trim().len() <= 4096
                    && !path.trim().chars().any(char::is_control) =>
            {
                paths.push(path.trim().to_owned())
            }
            _ => failed = true,
        }
    }
    paths.sort();
    paths.dedup();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|time| time.as_secs());
    for path in paths {
        // cat also bounds reads of misconfigured FIFOs or special files by the deadline.
        match command("cat", &["--", &path], deadline).and_then(|text| {
            leases(
                &text,
                now.ok_or_else(|| anyhow!("clock before epoch"))?,
                &state.lan,
            )
        }) {
            Ok(rows) => {
                observed = true;
                merge(&mut devices, rows);
            }
            Err(_) => failed = true,
        }
    }
    // iproute2's unspecified family dumps both IPv4 and IPv6 neighbours.
    match command(
        "ip",
        &["-j", "neigh", "show", "dev", &state.lan.device],
        deadline,
    )
    .and_then(|text| neighbors(&text, &state.lan))
    {
        Ok(rows) => {
            observed = true;
            merge(&mut devices, rows);
        }
        Err(_) => failed = true,
    }
    Ok(finish(devices, observed, failed))
}

fn finish(
    devices: BTreeMap<MacAddress, LanDevice>,
    observed: bool,
    failed: bool,
) -> LanDeviceInventory {
    LanDeviceInventory {
        devices: devices.into_values().collect(),
        discovery: if !observed {
            Discovery::Unavailable
        } else if failed {
            Discovery::Partial
        } else {
            Discovery::Complete
        },
    }
}

fn merge(devices: &mut BTreeMap<MacAddress, LanDevice>, rows: Vec<LanDevice>) {
    for row in rows {
        let device = devices.entry(row.mac).or_insert_with(|| LanDevice {
            mac: row.mac,
            name: None,
            addresses: vec![],
        });
        if let Some(name) = row.name {
            // Deterministic even when lease files disagree or change order.
            if device.name.as_ref().is_none_or(|old| name < *old) {
                device.name = Some(name);
            }
        }
        device.addresses.extend(row.addresses);
        device.addresses.sort_unstable();
        device.addresses.dedup();
    }
}

fn lease_options(text: &str) -> Result<Vec<Option<String>>> {
    ensure!(
        text.len() <= MAX_BYTES && text.lines().count() <= MAX_ROWS,
        "UCI output limit"
    );
    let sections: Vec<_> = text
        .lines()
        .filter_map(|line| line.split_once('='))
        .filter(|(_, value)| *value == "dnsmasq" || *value == "'dnsmasq'")
        .map(|(key, _)| key)
        .collect();
    ensure!(sections.len() <= MAX_FILES, "too many dnsmasq instances");
    let mut options = Vec::new();
    for section in sections {
        ensure!(
            section.starts_with("dhcp.")
                && section
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._@[]-".contains(&b)),
            "invalid UCI section"
        );
        let option = format!("{section}.leasefile");
        if text
            .lines()
            .filter_map(|line| line.split_once('='))
            .any(|(key, _)| key == option)
        {
            options.push(Some(option));
        } else {
            options.push(None);
        }
    }
    if options.is_empty() {
        options.push(None);
    }
    Ok(options)
}

fn in_lan(ip: IpAddr, lan: &LanContext) -> bool {
    if ip.is_unspecified() || ip.is_loopback() || ip.is_multicast() {
        return false;
    }
    match ip {
        IpAddr::V4(ip) => {
            lan.subnet.contains(&ip)
                && ip != lan.subnet.network()
                && ip != lan.subnet.broadcast()
                && ip != lan.address
        }
        IpAddr::V6(ip) => ip.to_ipv4_mapped().is_none(),
    }
}

fn hostname(value: &str) -> Option<String> {
    let name: String = value.chars().filter(|c| !c.is_control() && !matches!(*c, '\u{200e}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'))
        .scan(0, |bytes, c| { *bytes += c.len_utf8(); (*bytes <= MAX_NAME).then_some(c) }).collect();
    (!name.is_empty() && name != "*").then_some(name)
}

fn leases(text: &str, now: u64, lan: &LanContext) -> Result<Vec<LanDevice>> {
    ensure!(
        text.len() <= MAX_BYTES && text.lines().count() <= MAX_ROWS,
        "lease input limit"
    );
    Ok(text
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_ascii_whitespace();
            let expires = fields.next()?.parse::<u64>().ok()?;
            let mac = fields.next()?.parse::<MacAddress>().ok()?;
            let ip = fields.next()?.parse::<IpAddr>().ok()?;
            let name = fields.next()?;
            fields.next()?; // dnsmasq client-id, including '*'.
            if fields.next().is_some()
                || (expires != 0 && expires <= now)
                || !ip.is_ipv4()
                || !in_lan(ip, lan)
            {
                return None;
            }
            Some(LanDevice {
                mac,
                name: hostname(name),
                addresses: vec![ip],
            })
        })
        .collect())
}

fn neighbors(text: &str, lan: &LanContext) -> Result<Vec<LanDevice>> {
    ensure!(text.len() <= MAX_BYTES, "neighbor input limit");
    let entries: Vec<Value> = serde_json::from_str(text)?;
    ensure!(entries.len() <= MAX_ROWS, "neighbor row limit");
    Ok(entries
        .iter()
        .filter_map(|entry| {
            if let Some(dev) = entry.get("dev")
                && dev.as_str()? != lan.device
            {
                return None;
            }
            let states = entry.get("state")?.as_array()?;
            if states.is_empty()
                || states.iter().any(|state| {
                    state.as_str().is_none_or(|s| {
                        s.eq_ignore_ascii_case("FAILED") || s.eq_ignore_ascii_case("INCOMPLETE")
                    })
                })
            {
                return None;
            }
            let ip = entry.get("dst")?.as_str()?.parse::<IpAddr>().ok()?;
            let mac = entry.get("lladdr")?.as_str()?.parse::<MacAddress>().ok()?;
            in_lan(ip, lan).then_some(LanDevice {
                mac,
                name: None,
                addresses: vec![ip],
            })
        })
        .collect())
}

// Only native local uci/ip/cat executables: no shell, SSH, probes, or logging.
// Capture both pipes concurrently, retaining bounded stderr in command errors.
fn command(program: &str, args: &[&str], deadline: Instant) -> Result<String> {
    ensure!(Instant::now() < deadline, "discovery deadline");
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let (tx, rx) = mpsc::channel();
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("stdout pipe missing"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("stderr pipe missing"))?;
    for (is_stdout, pipe) in [
        (true, Box::new(stdout) as Box<dyn Read + Send>),
        (false, Box::new(stderr) as Box<dyn Read + Send>),
    ] {
        let tx = tx.clone();
        thread::spawn(move || {
            let mut bytes = Vec::new();
            let result = pipe
                .take((MAX_BYTES + 1) as u64)
                .read_to_end(&mut bytes)
                .map(|_| bytes);
            let _ = tx.send((is_stdout, result));
        });
    }
    drop(tx);
    let result = (|| {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        for _ in 0..2 {
            let (is_stdout, bytes) =
                rx.recv_timeout(deadline.saturating_duration_since(Instant::now()))?;
            let bytes = bytes?;
            ensure!(bytes.len() <= MAX_BYTES, "discovery output limit");
            if is_stdout {
                stdout = bytes;
            } else {
                stderr = bytes;
            }
        }
        loop {
            if let Some(status) = child.try_wait()? {
                ensure!(
                    status.success(),
                    "{program} failed ({status}): {}",
                    String::from_utf8_lossy(&stderr)
                );
                return Ok(String::from_utf8(stdout)?);
            }
            ensure!(Instant::now() < deadline, "discovery deadline");
            thread::sleep(Duration::from_millis(5));
        }
    })();
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lan() -> LanContext {
        LanContext {
            device: "br-lan".into(),
            address: "192.168.1.1".parse().unwrap(),
            subnet: "192.168.1.0/24".parse().unwrap(),
        }
    }

    #[test]
    fn leases_filter_expiry_invalid_mac_and_non_lan_addresses() {
        let text = "0 02:00:00:00:00:01 192.168.1.2 * *\n101 02:00:00:00:00:02 192.168.1.3 laptop *\n100 02:00:00:00:00:03 192.168.1.4 expired *\n99 02:00:00:00:00:04 192.168.1.5 expired *\n0 01:00:00:00:00:01 192.168.1.6 multicast *\n0 00:00:00:00:00:00 192.168.1.7 zero *\n0 02:00:00:00:00:05 203.0.113.1 wan *\n0 02:00:00:00:00:06 fe80::1 v6 *\nbroken\n";
        let rows = leases(text, 100, &lan()).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, None);
        assert_eq!(rows[1].name.as_deref(), Some("laptop"));
    }

    #[test]
    fn neighbors_are_dual_stack_scoped_and_exclude_failed_or_malformed() {
        let rows = neighbors(
            r#"[
            {"dst":"192.168.1.2","lladdr":"02:00:00:00:00:01","state":["STALE"]},
            {"dst":"fe80::2","lladdr":"02:00:00:00:00:01","state":["REACHABLE"],"dev":"br-lan"},
            {"dst":"fe80::3","lladdr":"02:00:00:00:00:02","state":["STALE"],"dev":"wan"},
            {"dst":"203.0.113.2","lladdr":"02:00:00:00:00:02","state":["REACHABLE"]},
            {"dst":"192.168.1.3","lladdr":"02:00:00:00:00:02","state":["FAILED"]},
            {"dst":"192.168.1.4","lladdr":"02:00:00:00:00:02","state":["INCOMPLETE"]},
            {"dst":"bad","lladdr":"02:00:00:00:00:02","state":["STALE"]},
            {"dst":"192.168.1.5","lladdr":"ff:ff:ff:ff:ff:ff","state":["STALE"]},
            {},null
        ]"#,
            &lan(),
        )
        .unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows[1].addresses[0].is_ipv6());
        assert!(neighbors("not JSON", &lan()).is_err());
    }

    #[test]
    fn inventory_merges_deduplicates_and_retains_unobserved_exclusions() {
        let mut devices = BTreeMap::new();
        let mac = "02:00:00:00:00:09".parse().unwrap();
        merge(
            &mut devices,
            vec![LanDevice {
                mac,
                name: None,
                addresses: vec![],
            }],
        );
        let text = "0 02:00:00:00:00:01 192.168.1.3 laptop *\n0 02:00:00:00:00:01 192.168.1.2 laptop *\n0 02:00:00:00:00:01 192.168.1.3 laptop *\n";
        merge(&mut devices, leases(text, 1, &lan()).unwrap());
        let inventory = serde_json::to_value(finish(devices, true, true)).unwrap();
        assert_eq!(
            inventory,
            serde_json::json!({"discovery":"partial","devices":[
                {"mac":"02:00:00:00:00:01","name":"laptop","addresses":["192.168.1.2","192.168.1.3"]},
                {"mac":"02:00:00:00:00:09","name":null,"addresses":[]}
            ]})
        );
    }

    #[test]
    fn discovery_distinguishes_successful_empty_from_failure() {
        assert_eq!(
            finish(BTreeMap::new(), true, false).discovery,
            Discovery::Complete
        );
        assert_eq!(
            finish(BTreeMap::new(), false, true).discovery,
            Discovery::Unavailable
        );
        let mut devices = BTreeMap::new();
        merge(
            &mut devices,
            vec![LanDevice {
                mac: "02:00:00:00:00:09".parse().unwrap(),
                name: None,
                addresses: vec![],
            }],
        );
        let inventory = finish(devices, false, true);
        assert_eq!(inventory.discovery, Discovery::Unavailable);
        assert_eq!(inventory.devices.len(), 1);
    }

    #[test]
    fn uci_selects_only_dnsmasq_lease_options() {
        assert_eq!(lease_options("dhcp.main=dnsmasq\ndhcp.main.leasefile='/tmp/custom leases'\ndhcp.@dnsmasq[1]=dnsmasq\ndhcp.@dnsmasq[1].leasefile='/tmp/other'\ndhcp.lan=dhcp\ndhcp.lan.leasefile='/tmp/wrong'\ndhcp.default=dnsmasq\n").unwrap(), vec![Some("dhcp.main.leasefile".into()), Some("dhcp.@dnsmasq[1].leasefile".into()), None]);
        assert_eq!(lease_options("dhcp.main=dnsmasq\n").unwrap(), vec![None]);
    }

    #[test]
    fn bounds_and_hostname_sanitizing() {
        assert_eq!(hostname("a\0b\u{202e}c"), Some("abc".into()));
        assert!(hostname(&"é".repeat(100)).unwrap().len() <= MAX_NAME);
        assert!(leases(&"\n".repeat(MAX_ROWS + 1), 0, &lan()).is_err());
        assert!(neighbors(&" ".repeat(MAX_BYTES + 1), &lan()).is_err());
    }

    #[test]
    fn a_new_snapshot_tracks_ip_changes_without_caching() {
        let old = leases("0 02:00:00:00:00:01 192.168.1.2 * *", 0, &lan()).unwrap();
        let new = leases("0 02:00:00:00:00:01 192.168.1.3 * *", 0, &lan()).unwrap();
        assert_ne!(old[0].addresses, new[0].addresses);
        assert_eq!(old[0].mac.to_string(), new[0].mac.to_string());
    }
}
