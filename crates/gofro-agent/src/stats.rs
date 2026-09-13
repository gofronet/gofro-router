use std::{
    collections::VecDeque,
    fs,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::Serialize;

const HISTORY_LIMIT: usize = 90;

#[derive(Clone, Debug, Default, Serialize)]
pub(crate) struct LiveStats {
    rx_bps: u64,
    tx_bps: u64,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct HistoryPoint {
    timestamp: u64,
    rx_bps: u64,
    tx_bps: u64,
}

#[derive(Default)]
pub(crate) struct StatsTracker {
    previous: Option<(u64, u64, u64)>,
    history: VecDeque<HistoryPoint>,
}

impl StatsTracker {
    pub(crate) fn sample(&mut self, (rx, tx): (u64, u64)) -> (LiveStats, Vec<HistoryPoint>) {
        let timestamp = unix_time();
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
        let stats = LiveStats { rx_bps, tx_bps };
        if self
            .history
            .back()
            .is_some_and(|last| last.timestamp == timestamp)
        {
            self.history.pop_back();
        }
        self.history.push_back(HistoryPoint {
            timestamp,
            rx_bps,
            tx_bps,
        });
        if self.history.len() > HISTORY_LIMIT {
            self.history.pop_front();
        }
        (stats, self.history.iter().cloned().collect())
    }
}

pub(crate) fn interface_traffic(interface: &str) -> Option<(u64, u64)> {
    interface_traffic_at(Path::new("/sys/class/net"), interface)
}

fn interface_traffic_at(root: &Path, interface: &str) -> Option<(u64, u64)> {
    let stats = root.join(interface).join("statistics");
    Some((
        fs::read_to_string(stats.join("rx_bytes"))
            .ok()?
            .trim()
            .parse()
            .ok()?,
        fs::read_to_string(stats.join("tx_bytes"))
            .ok()?
            .trim()
            .parse()
            .ok()?,
    ))
}

fn unix_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_vpn_receive_as_download_and_transmit_as_upload() {
        let root = std::env::temp_dir().join(format!("gofro-stats-{}", std::process::id()));
        let stats = root.join("gt0/statistics");
        fs::create_dir_all(&stats).unwrap();
        fs::write(stats.join("rx_bytes"), "200\n").unwrap();
        fs::write(stats.join("tx_bytes"), "100\n").unwrap();

        assert_eq!(interface_traffic_at(&root, "gt0"), Some((200, 100)));
        fs::remove_dir_all(root).unwrap();
    }
}
