use std::{collections::HashMap, fs, process::Command};

#[derive(Clone, Debug, Default)]
pub(crate) struct DeviceReading {
    pub(crate) mac: String,
    pub(crate) ip: Option<String>,
    pub(crate) hostname: Option<String>,
    pub(crate) signal_dbm: Option<i32>,
    pub(crate) rx_bytes: u64,
    pub(crate) tx_bytes: u64,
    pub(crate) rx_bitrate_mbps: Option<f64>,
    pub(crate) tx_bitrate_mbps: Option<f64>,
    pub(crate) connected_seconds: u64,
    pub(crate) inactive_ms: u64,
}

pub(crate) fn devices(fallback_interface: &str) -> Vec<DeviceReading> {
    let interfaces = Command::new("iw")
        .arg("dev")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| parse_ap_interfaces(&String::from_utf8_lossy(&output.stdout)))
        .filter(|interfaces| !interfaces.is_empty())
        .unwrap_or_else(|| vec![fallback_interface.to_owned()]);
    let leases = dhcp_leases();

    interfaces
        .into_iter()
        .flat_map(|interface| {
            Command::new("iw")
                .args(["dev", &interface, "station", "dump"])
                .output()
                .ok()
                .filter(|output| output.status.success())
                .map_or_else(Vec::new, |output| {
                    parse_devices(&String::from_utf8_lossy(&output.stdout), &leases)
                })
        })
        .collect()
}

fn parse_ap_interfaces(output: &str) -> Vec<String> {
    let mut current = None;
    let mut interfaces = Vec::new();
    for line in output.lines().map(str::trim) {
        if let Some(interface) = line.strip_prefix("Interface ") {
            current = interface.split_whitespace().next().map(str::to_owned);
        } else if line == "type AP"
            && let Some(interface) = current.take()
        {
            interfaces.push(interface);
        }
    }
    interfaces
}

fn parse_devices(
    output: &str,
    leases: &HashMap<String, (String, Option<String>)>,
) -> Vec<DeviceReading> {
    let mut devices = Vec::new();
    let mut current: Option<DeviceReading> = None;
    for line in output.lines().map(str::trim) {
        if let Some(station) = line.strip_prefix("Station ") {
            if let Some(device) = current.take() {
                devices.push(device);
            }
            let mac = station
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_ascii_lowercase();
            let lease = leases.get(&mac);
            current = Some(DeviceReading {
                mac,
                ip: lease.map(|(ip, _)| ip.clone()),
                hostname: lease.and_then(|(_, hostname)| hostname.clone()),
                ..DeviceReading::default()
            });
            continue;
        }

        let Some(device) = current.as_mut() else {
            continue;
        };
        if let Some(value) = station_value::<u64>(line, "inactive time:") {
            device.inactive_ms = value;
        } else if let Some(value) = station_value::<u64>(line, "rx bytes:") {
            device.rx_bytes = value;
        } else if let Some(value) = station_value::<u64>(line, "tx bytes:") {
            device.tx_bytes = value;
        } else if let Some(value) = station_value::<i32>(line, "signal:") {
            device.signal_dbm = Some(value);
        } else if let Some(value) = station_value::<f64>(line, "rx bitrate:") {
            device.rx_bitrate_mbps = Some(value);
        } else if let Some(value) = station_value::<f64>(line, "tx bitrate:") {
            device.tx_bitrate_mbps = Some(value);
        } else if let Some(value) = station_value::<u64>(line, "connected time:") {
            device.connected_seconds = value;
        }
    }
    if let Some(device) = current {
        devices.push(device);
    }
    devices
}

fn station_value<T>(line: &str, prefix: &str) -> Option<T>
where
    T: std::str::FromStr,
{
    line.strip_prefix(prefix)?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

fn dhcp_leases() -> HashMap<String, (String, Option<String>)> {
    let Ok(contents) = fs::read_to_string("/tmp/dhcp.leases") else {
        return HashMap::new();
    };
    contents
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            fields.next()?;
            let mac = fields.next()?.to_ascii_lowercase();
            let ip = fields.next()?.to_owned();
            let hostname = fields
                .next()
                .filter(|hostname| *hostname != "*")
                .map(str::to_owned);
            Some((mac, (ip, hostname)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_connected_device_details() {
        let output = concat!(
            "Station aa:bb:cc:dd:ee:ff (on wlan0)\n",
            "\tinactive time:\t120 ms\n",
            "\trx bytes:\t2048\n",
            "\ttx bytes:\t4096\n",
            "\tsignal:\t-47 dBm\n",
            "\ttx bitrate:\t72.2 MBit/s\n",
            "\trx bitrate:\t58.5 MBit/s\n",
            "\tconnected time:\t90 seconds\n",
        );
        let leases = HashMap::from([(
            "aa:bb:cc:dd:ee:ff".into(),
            ("10.203.1.2".into(), Some("phone".into())),
        )]);

        let device = parse_devices(output, &leases).remove(0);
        assert_eq!(device.ip.as_deref(), Some("10.203.1.2"));
        assert_eq!(device.hostname.as_deref(), Some("phone"));
        assert_eq!(device.signal_dbm, Some(-47));
        assert_eq!(device.tx_bytes, 4096);
        assert_eq!(device.connected_seconds, 90);
    }

    #[test]
    fn finds_all_access_point_interfaces() {
        let output = concat!(
            "phy#1\n",
            "\tInterface phy1-ap0\n",
            "\t\ttype AP\n",
            "phy#0\n",
            "\tInterface phy0-sta0\n",
            "\t\ttype managed\n",
            "\tInterface phy0-ap0\n",
            "\t\ttype AP\n",
        );

        assert_eq!(parse_ap_interfaces(output), ["phy1-ap0", "phy0-ap0"]);
    }
}
