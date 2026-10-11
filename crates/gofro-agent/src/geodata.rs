mod wire;

use std::{
    collections::{HashMap, HashSet},
    fs::File,
    net::Ipv4Addr,
    path::Path,
    sync::{Arc, Mutex, Weak},
};

use anyhow::{Context, Result, bail};
use ipnet::Ipv4Net;
use regex::{Regex, RegexBuilder};

use crate::{
    model::{DomainMatch, IpMatch, RoutingConfig, RoutingMode},
    resources::ResourceLimits,
};
use wire::{Range, Reader, Value};

#[derive(Clone, Debug, Default)]
pub(crate) struct GeoData {
    source: Option<Arc<Catalog>>,
    sites: HashMap<String, Arc<Site>>,
    ips: HashMap<String, Arc<Vec<Ipv4Net>>>,
}

#[derive(Debug)]
struct Catalog {
    sites: Mutex<IndexedFile<Site>>,
    ips: Mutex<IndexedFile<Vec<Ipv4Net>>>,
    limits: ResourceLimits,
}

#[derive(Debug)]
struct IndexedFile<T> {
    // Keep the release's inode open, including across an updater symlink switch.
    file: File,
    entries: HashMap<String, Range>,
    cache: HashMap<String, Weak<T>>,
}

#[derive(Debug, Default)]
struct Site {
    full: HashSet<Box<str>>,
    roots: HashSet<Box<str>>,
    keywords: Vec<Box<str>>,
    regexes: Vec<Regex>,
}

impl Site {
    fn matches(&self, domain: &str) -> bool {
        self.full.contains(domain)
            || self.roots.contains(domain)
            || domain
                .match_indices('.')
                .any(|(offset, _)| self.roots.contains(&domain[offset + 1..]))
            || self
                .keywords
                .iter()
                .any(|keyword| domain.contains(keyword.as_ref()))
            || self.regexes.iter().any(|regex| regex.is_match(domain))
    }
}

impl<T> IndexedFile<T> {
    fn open(path: &Path, geoip: bool) -> Result<Self> {
        let file =
            File::open(path).with_context(|| format!("failed to read {}", path.display()))?;
        let mut reader = Reader::new(&file)?;
        let mut entries = HashMap::new();
        while let Some((field, value)) = reader.field()? {
            if field != 1 {
                continue;
            }
            let Value::Bytes(range) = value else {
                bail!("invalid geodata entry");
            };
            let (tag, inverse) = reader.within(range, |reader| {
                let mut tag = String::new();
                let mut inverse = false;
                while let Some((field, value)) = reader.field()? {
                    match (field, value) {
                        (1, Value::Bytes(value)) => {
                            tag = reader.string(value, 64)?.to_ascii_lowercase()
                        }
                        (3, Value::Integer(value)) if geoip => inverse = value != 0,
                        (3, _) if geoip => bail!("invalid GeoIP inverse match"),
                        (1, _) => bail!("invalid geodata tag"),
                        _ => {}
                    }
                }
                Ok((tag, inverse))
            })?;
            if !inverse {
                entries.insert(tag, range);
            }
        }
        drop(reader);
        Ok(Self {
            file,
            entries,
            cache: HashMap::new(),
        })
    }

    fn get(
        &mut self,
        tag: &str,
        decode: impl FnOnce(&mut Reader<'_>) -> Result<T>,
    ) -> Result<Option<Arc<T>>> {
        if let Some(value) = self.cache.get(tag).and_then(Weak::upgrade) {
            return Ok(Some(value));
        }
        let Some(range) = self.entries.get(tag).copied() else {
            return Ok(None);
        };
        let value = Arc::new(Reader::new(&self.file)?.within(range, decode)?);
        // Weak references reuse active tags without keeping removed rules alive.
        self.cache.insert(tag.to_owned(), Arc::downgrade(&value));
        Ok(Some(value))
    }
}

impl GeoData {
    #[cfg(test)]
    pub(crate) fn load(geosite: &Path, geoip: &Path) -> Result<Self> {
        Self::load_with_limits(geosite, geoip, ResourceLimits::STANDARD)
    }

    pub(crate) fn load_with_limits(
        geosite: &Path,
        geoip: &Path,
        limits: ResourceLimits,
    ) -> Result<Self> {
        Ok(Self {
            source: Some(Arc::new(Catalog {
                sites: Mutex::new(IndexedFile::open(geosite, false)?),
                ips: Mutex::new(IndexedFile::open(geoip, true)?),
                limits,
            })),
            ..Self::default()
        })
    }

    pub(crate) fn select(self: &Arc<Self>, config: &RoutingConfig) -> Result<Arc<Self>> {
        let Some(source) = &self.source else {
            return Ok(Arc::clone(self));
        };
        let mut selected = Self {
            source: Some(Arc::clone(source)),
            ..Self::default()
        };
        let mut site_bytes = 0_u64;
        let mut regex_count = 0_usize;
        let mut ip_intervals = 0_usize;
        if config.mode == RoutingMode::Rules {
            for rule in &config.domain_rules {
                if rule.enabled
                    && let DomainMatch::GeoSite { value } = &rule.matcher
                {
                    if selected.sites.contains_key(value) {
                        continue;
                    }
                    let mut sites = source
                        .sites
                        .lock()
                        .map_err(|_| anyhow::anyhow!("GeoSite lock poisoned"))?;
                    let range = sites.entries.get(value).with_context(|| {
                        format!("GeoSite-тег {value} отсутствует в geosite.dat")
                    })?;
                    site_bytes = site_bytes.saturating_add(range.len);
                    anyhow::ensure!(
                        site_bytes <= source.limits.geosite_bytes as u64,
                        "GeoSite-правила превышают бюджет compact-профиля; уменьшите список тегов или используйте устройство с большей RAM"
                    );
                    let site = sites
                        .get(value, |reader| decode_site(reader, source.limits))?
                        .with_context(|| {
                            format!("GeoSite-тег {value} отсутствует в geosite.dat")
                        })?;
                    regex_count = regex_count.saturating_add(site.regexes.len());
                    anyhow::ensure!(
                        regex_count <= source.limits.regex_count,
                        "слишком много GeoSite regex для compact-профиля (максимум {})",
                        source.limits.regex_count
                    );
                    selected.sites.insert(value.clone(), site);
                }
            }
            for rule in &config.ip_rules {
                if rule.enabled
                    && let IpMatch::GeoIp { value } = &rule.matcher
                {
                    if !selected.ips.contains_key(value) {
                        let networks = source
                            .ips
                            .lock()
                            .map_err(|_| anyhow::anyhow!("GeoIP lock poisoned"))?
                            .get(value, decode_ip)?
                            .with_context(|| {
                                format!("GeoIP-тег {value} отсутствует в geoip.dat")
                            })?;
                        selected.ips.insert(value.clone(), networks);
                    }
                    let networks = selected.ip_networks(value);
                    let intervals = usize::from(!networks.is_empty())
                        + networks
                            .windows(2)
                            .filter(|pair| {
                                u32::from(pair[1].network())
                                    > u32::from(pair[0].broadcast()).saturating_add(1)
                            })
                            .count();
                    ip_intervals = ip_intervals.saturating_add(intervals);
                    anyhow::ensure!(
                        ip_intervals <= source.limits.geoip_intervals,
                        "GeoIP-правила превышают бюджет compact-профиля: {ip_intervals} интервалов при лимите {}",
                        source.limits.geoip_intervals
                    );
                }
            }
        }
        Ok(Arc::new(selected))
    }

    pub(crate) fn has_site(&self, tag: &str) -> bool {
        self.sites.contains_key(tag)
            || self.source.as_ref().is_some_and(|source| {
                source
                    .sites
                    .lock()
                    .is_ok_and(|sites| sites.entries.contains_key(tag))
            })
    }

    pub(crate) fn has_ip(&self, tag: &str) -> bool {
        self.ips.contains_key(tag)
            || self.source.as_ref().is_some_and(|source| {
                source
                    .ips
                    .lock()
                    .is_ok_and(|ips| ips.entries.contains_key(tag))
            })
    }

    pub(crate) fn matches_site(&self, tag: &str, domain: &str) -> bool {
        self.sites.get(tag).is_some_and(|site| site.matches(domain))
    }

    pub(crate) fn matches_ip(&self, tag: &str, ip: Ipv4Addr) -> bool {
        let networks = self.ip_networks(tag);
        let index = networks.partition_point(|network| network.network() <= ip);
        index > 0 && networks[index - 1].contains(&ip)
    }

    pub(crate) fn ip_networks(&self, tag: &str) -> &[Ipv4Net] {
        self.ips
            .get(tag)
            .map_or(&[], |networks| networks.as_slice())
    }
}

fn decode_site(reader: &mut Reader<'_>, limits: ResourceLimits) -> Result<Site> {
    let mut site = Site::default();
    while let Some((field, value)) = reader.field()? {
        if field != 2 {
            continue;
        }
        let Value::Bytes(range) = value else {
            bail!("invalid GeoSite domain");
        };
        let (kind, value) = reader.within(range, |reader| {
            let mut kind = 0;
            let mut domain = String::new();
            while let Some((field, value)) = reader.field()? {
                match (field, value) {
                    (1, Value::Integer(value)) => kind = value,
                    (2, Value::Bytes(value)) => domain = reader.string(value, 128 * 1024)?,
                    (1 | 2, _) => bail!("invalid GeoSite domain field"),
                    _ => {}
                }
            }
            Ok((kind, domain))
        })?;
        match kind {
            0 => site
                .keywords
                .push(value.to_ascii_lowercase().into_boxed_str()),
            1 => {
                anyhow::ensure!(
                    site.regexes.len() < limits.regex_count,
                    "слишком много GeoSite regex для compact-профиля (максимум {})",
                    limits.regex_count
                );
                site.regexes.push(
                    RegexBuilder::new(&value)
                        .size_limit(limits.regex_size_bytes)
                        .dfa_size_limit(limits.regex_cache_bytes)
                        .build()
                        .with_context(|| format!("invalid GeoSite regex {value}"))?,
                );
            }
            2 => {
                site.roots
                    .insert(value.to_ascii_lowercase().into_boxed_str());
            }
            3 => {
                site.full
                    .insert(value.to_ascii_lowercase().into_boxed_str());
            }
            kind => bail!("unsupported GeoSite domain type {kind}"),
        }
    }
    site.full.shrink_to_fit();
    site.roots.shrink_to_fit();
    site.keywords.shrink_to_fit();
    site.regexes.shrink_to_fit();
    Ok(site)
}

fn decode_ip(reader: &mut Reader<'_>) -> Result<Vec<Ipv4Net>> {
    let mut networks = Vec::new();
    while let Some((field, value)) = reader.field()? {
        if field != 2 {
            continue;
        }
        let Value::Bytes(range) = value else {
            bail!("invalid GeoIP CIDR");
        };
        let network = reader.within(range, |reader| {
            let mut address = None;
            let mut prefix = 0;
            while let Some((field, value)) = reader.field()? {
                match (field, value) {
                    (1, Value::Bytes(value)) => {
                        // IPv6 is skipped in-place, without allocating an address Vec.
                        address = if value.len == 4 {
                            Some(Ipv4Addr::from(reader.bytes::<4>(value)?))
                        } else {
                            None
                        };
                    }
                    (2, Value::Integer(value)) => prefix = value,
                    (1 | 2, _) => bail!("invalid GeoIP CIDR field"),
                    _ => {}
                }
            }
            Ok(address.filter(|_| prefix <= 32).map(|address| {
                Ipv4Net::new(address, prefix as u8)
                    .expect("prefix was checked")
                    .trunc()
            }))
        })?;
        if let Some(network) = network {
            networks.push(network);
        }
    }
    // Disjoint, sorted ranges make lookup logarithmic and reduce nft input too.
    aggregate_ipv4(&mut networks);
    Ok(networks)
}

fn aggregate_ipv4(networks: &mut Vec<Ipv4Net>) {
    networks.sort_unstable();
    let mut written = 0;
    for read in 0..networks.len() {
        let mut network = networks[read];
        if written > 0 && networks[written - 1].contains(&network) {
            continue;
        }
        while written > 0 {
            let previous = networks[written - 1];
            let Some(parent) = network.supernet() else {
                break;
            };
            if previous.prefix_len() != network.prefix_len() || !parent.contains(&previous) {
                break;
            }
            written -= 1;
            network = parent;
        }
        networks[written] = network;
        written += 1;
    }
    networks.truncate(written);
    networks.shrink_to_fit();
}

#[cfg(test)]
mod tests;
