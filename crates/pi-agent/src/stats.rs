use std::{
    collections::{HashMap, VecDeque},
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::Serialize;
use tunnel_core::PeerStatus;

use crate::wifi::DeviceReading;

const HISTORY_LIMIT: usize = 90;

#[derive(Clone, Debug, Default, Serialize)]
pub(crate) struct LiveStats {
    rx_bps: u64,
    tx_bps: u64,
    load_percent: f64,
    memory_percent: f64,
    temperature_c: Option<f64>,
    uptime_seconds: u64,
    wifi_clients: usize,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct HistoryPoint {
    timestamp: u64,
    rx_bps: u64,
    tx_bps: u64,
    load_percent: f64,
    memory_percent: f64,
    temperature_c: Option<f64>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct DeviceStatus {
    mac: String,
    ip: Option<String>,
    hostname: Option<String>,
    signal_dbm: Option<i32>,
    rx_bytes: u64,
    tx_bytes: u64,
    rx_bps: u64,
    tx_bps: u64,
    rx_bitrate_mbps: Option<f64>,
    tx_bitrate_mbps: Option<f64>,
    connected_seconds: u64,
    inactive_ms: u64,
}

#[derive(Default)]
pub(crate) struct StatsTracker {
    previous: Option<(u64, u64, u64)>,
    previous_cpu: Option<(u64, u64)>,
    device_previous: HashMap<String, (u64, u64, u64)>,
    history: VecDeque<HistoryPoint>,
}

impl StatsTracker {
    pub(crate) fn sample(
        &mut self,
        peer: Option<&PeerStatus>,
        readings: Vec<DeviceReading>,
    ) -> (LiveStats, Vec<HistoryPoint>, Vec<DeviceStatus>) {
        let timestamp = unix_time();
        let rx = peer.map_or(0, |peer| peer.rx_bytes);
        let tx = peer.map_or(0, |peer| peer.tx_bytes);
        let (rx_bps, tx_bps) = self.previous.map_or((0, 0), |(at, old_rx, old_tx)| {
            let elapsed = timestamp.saturating_sub(at);
            if elapsed == 0 {
                return self
                    .history
                    .back()
                    .map_or((0, 0), |point| (point.rx_bps, point.tx_bps));
            }
            (
                rx.saturating_sub(old_rx).saturating_mul(8) / elapsed,
                tx.saturating_sub(old_tx).saturating_mul(8) / elapsed,
            )
        });
        self.previous = Some((timestamp, rx, tx));

        let load_percent = cpu_counters().map_or(0.0, |(idle, total)| {
            let percent = self.previous_cpu.map_or(0.0, |(old_idle, old_total)| {
                let elapsed = total.saturating_sub(old_total);
                if elapsed == 0 {
                    0.0
                } else {
                    elapsed.saturating_sub(idle.saturating_sub(old_idle)) as f64 * 100.0
                        / elapsed as f64
                }
            });
            self.previous_cpu = Some((idle, total));
            percent.clamp(0.0, 100.0)
        });
        let devices: Vec<_> = readings
            .into_iter()
            .map(|device| {
                let (rx_bps, tx_bps) =
                    self.device_previous
                        .get(&device.mac)
                        .map_or((0, 0), |(at, old_rx, old_tx)| {
                            let elapsed = timestamp.saturating_sub(*at);
                            (
                                device
                                    .rx_bytes
                                    .saturating_sub(*old_rx)
                                    .saturating_mul(8)
                                    .checked_div(elapsed)
                                    .unwrap_or(0),
                                device
                                    .tx_bytes
                                    .saturating_sub(*old_tx)
                                    .saturating_mul(8)
                                    .checked_div(elapsed)
                                    .unwrap_or(0),
                            )
                        });
                self.device_previous.insert(
                    device.mac.clone(),
                    (timestamp, device.rx_bytes, device.tx_bytes),
                );
                DeviceStatus {
                    mac: device.mac,
                    ip: device.ip,
                    hostname: device.hostname,
                    signal_dbm: device.signal_dbm,
                    rx_bytes: device.rx_bytes,
                    tx_bytes: device.tx_bytes,
                    rx_bps,
                    tx_bps,
                    rx_bitrate_mbps: device.rx_bitrate_mbps,
                    tx_bitrate_mbps: device.tx_bitrate_mbps,
                    connected_seconds: device.connected_seconds,
                    inactive_ms: device.inactive_ms,
                }
            })
            .collect();
        self.device_previous
            .retain(|mac, _| devices.iter().any(|device| device.mac == *mac));
        let stats = LiveStats {
            rx_bps,
            tx_bps,
            load_percent,
            memory_percent: memory_percent(),
            temperature_c: read_number("/sys/class/thermal/thermal_zone0/temp")
                .map(|value| value / 1000.0),
            uptime_seconds: read_number("/proc/uptime").unwrap_or(0.0) as u64,
            wifi_clients: devices.len(),
        };
        let point = HistoryPoint {
            timestamp,
            rx_bps,
            tx_bps,
            load_percent: stats.load_percent,
            memory_percent: stats.memory_percent,
            temperature_c: stats.temperature_c,
        };
        if self
            .history
            .back()
            .is_some_and(|last| last.timestamp == timestamp)
        {
            self.history.pop_back();
        }
        self.history.push_back(point);
        if self.history.len() > HISTORY_LIMIT {
            self.history.pop_front();
        }

        (stats, self.history.iter().cloned().collect(), devices)
    }
}

fn unix_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn read_number(path: &str) -> Option<f64> {
    fs::read_to_string(path)
        .ok()?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

fn cpu_counters() -> Option<(u64, u64)> {
    let contents = fs::read_to_string("/proc/stat").ok()?;
    let values: Vec<u64> = contents
        .lines()
        .next()?
        .split_whitespace()
        .skip(1)
        .map(str::parse)
        .collect::<std::result::Result<_, _>>()
        .ok()?;
    let idle = values.get(3)? + values.get(4).copied().unwrap_or(0);
    Some((idle, values.iter().sum()))
}

fn memory_percent() -> f64 {
    let Ok(contents) = fs::read_to_string("/proc/meminfo") else {
        return 0.0;
    };
    let mut total = 0.0_f64;
    let mut available = 0.0_f64;
    for line in contents.lines() {
        if let Some(value) = line.strip_prefix("MemTotal:") {
            total = value
                .split_whitespace()
                .next()
                .and_then(|value| value.parse().ok())
                .unwrap_or(0.0);
        } else if let Some(value) = line.strip_prefix("MemAvailable:") {
            available = value
                .split_whitespace()
                .next()
                .and_then(|value| value.parse().ok())
                .unwrap_or(0.0);
        }
    }
    if total == 0.0 {
        0.0
    } else {
        ((total - available) * 100.0 / total).clamp(0.0, 100.0)
    }
}
