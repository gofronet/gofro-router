use clap::ValueEnum;

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
pub(crate) enum MemoryProfile {
    #[default]
    Auto,
    Compact,
    Standard,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ResourceLimits {
    pub(crate) worker_threads: usize,
    pub(crate) blocking_threads: usize,
    pub(crate) stack_bytes: usize,
    pub(crate) dns_workers: usize,
    pub(crate) dns_tcp_connections: usize,
    pub(crate) fake_leases: usize,
    pub(crate) sqlite_cache_kib: usize,
    pub(crate) wal_pages: usize,
    pub(crate) geosite_bytes: usize,
    pub(crate) regex_count: usize,
    pub(crate) regex_size_bytes: usize,
    pub(crate) regex_cache_bytes: usize,
    pub(crate) geoip_intervals: usize,
}

impl ResourceLimits {
    pub(crate) const STANDARD: Self = Self {
        worker_threads: 2,
        blocking_threads: 24,
        stack_bytes: 512 * 1024,
        dns_workers: 16,
        dns_tcp_connections: 32,
        fake_leases: 32_768,
        sqlite_cache_kib: 256,
        wal_pages: 64,
        geosite_bytes: usize::MAX,
        regex_count: usize::MAX,
        regex_size_bytes: 10 * 1024 * 1024,
        regex_cache_bytes: 64 * 1024,
        geoip_intervals: usize::MAX,
    };

    pub(crate) const COMPACT: Self = Self {
        worker_threads: 1,
        blocking_threads: 16,
        stack_bytes: 256 * 1024,
        dns_workers: 8,
        dns_tcp_connections: 8,
        fake_leases: 8192,
        sqlite_cache_kib: 128,
        wal_pages: 32,
        geosite_bytes: 512 * 1024,
        regex_count: 8,
        regex_size_bytes: 128 * 1024,
        regex_cache_bytes: 16 * 1024,
        geoip_intervals: 16_384,
    };
}

impl MemoryProfile {
    pub(crate) fn resolve(self) -> ResourceLimits {
        let total = std::fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|info| {
                info.lines().find_map(|line| {
                    let mut fields = line.split_whitespace();
                    (fields.next()? == "MemTotal:")
                        .then(|| fields.next()?.parse().ok())
                        .flatten()
                })
            });
        self.for_memory(total)
    }

    fn for_memory(self, total_kib: Option<usize>) -> ResourceLimits {
        match self {
            Self::Compact => ResourceLimits::COMPACT,
            Self::Auto if total_kib.is_some_and(|total| total < 96 * 1024) => {
                ResourceLimits::COMPACT
            }
            _ => ResourceLimits::STANDARD,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_profile_covers_reserved_memory_and_explicit_overrides() {
        for total in [48 * 1024, 60 * 1024, 96 * 1024 - 1] {
            assert_eq!(
                MemoryProfile::Auto.for_memory(Some(total)).fake_leases,
                8192
            );
        }
        assert_eq!(
            MemoryProfile::Auto.for_memory(Some(96 * 1024)).fake_leases,
            32_768
        );
        assert_eq!(MemoryProfile::Auto.for_memory(None).fake_leases, 32_768);
        assert_eq!(
            MemoryProfile::Standard
                .for_memory(Some(48 * 1024))
                .fake_leases,
            32_768
        );
        assert_eq!(
            MemoryProfile::Compact
                .for_memory(Some(512 * 1024))
                .fake_leases,
            8192
        );
    }
}
