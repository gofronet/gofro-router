use std::{collections::HashMap, fs, net::Ipv4Addr, path::Path};

use anyhow::{Context, Result, bail};
use ipnet::Ipv4Net;
use prost::Message;
use regex::Regex;

#[derive(Clone, Debug, Default)]
pub(crate) struct GeoData {
    sites: HashMap<String, Vec<DomainPattern>>,
    ips: HashMap<String, Vec<Ipv4Net>>,
}

#[derive(Clone, Debug)]
enum DomainPattern {
    Keyword(String),
    Regex(Regex),
    Root(String),
    Full(String),
}

impl GeoData {
    pub(crate) fn load(geosite: &Path, geoip: &Path) -> Result<Self> {
        Ok(Self {
            sites: decode_geosite(
                &fs::read(geosite)
                    .with_context(|| format!("failed to read {}", geosite.display()))?,
            )?,
            ips: decode_geoip(
                &fs::read(geoip).with_context(|| format!("failed to read {}", geoip.display()))?,
            )?,
        })
    }

    pub(crate) fn has_site(&self, tag: &str) -> bool {
        self.sites.contains_key(tag)
    }

    pub(crate) fn has_ip(&self, tag: &str) -> bool {
        self.ips.contains_key(tag)
    }

    pub(crate) fn matches_site(&self, tag: &str, domain: &str) -> bool {
        self.sites.get(tag).is_some_and(|patterns| {
            patterns.iter().any(|pattern| match pattern {
                DomainPattern::Keyword(value) => domain.contains(value),
                DomainPattern::Regex(value) => value.is_match(domain),
                DomainPattern::Root(value) => {
                    domain == value
                        || domain
                            .strip_suffix(value)
                            .is_some_and(|prefix| prefix.ends_with('.'))
                }
                DomainPattern::Full(value) => domain == value,
            })
        })
    }

    pub(crate) fn matches_ip(&self, tag: &str, ip: Ipv4Addr) -> bool {
        self.ips
            .get(tag)
            .is_some_and(|networks| networks.iter().any(|network| network.contains(&ip)))
    }

    pub(crate) fn ip_networks(&self, tag: &str) -> &[Ipv4Net] {
        self.ips.get(tag).map_or(&[], Vec::as_slice)
    }
}

fn decode_geosite(bytes: &[u8]) -> Result<HashMap<String, Vec<DomainPattern>>> {
    let list = GeoSiteList::decode(bytes).context("invalid V2Ray geosite.dat")?;
    let mut sites = HashMap::with_capacity(list.entry.len());
    for site in list.entry {
        let mut patterns = Vec::with_capacity(site.domain.len());
        for domain in site.domain {
            let pattern = match domain.kind {
                0 => DomainPattern::Keyword(domain.value.to_ascii_lowercase()),
                1 => DomainPattern::Regex(
                    Regex::new(&domain.value)
                        .with_context(|| format!("invalid GeoSite regex {}", domain.value))?,
                ),
                2 => DomainPattern::Root(domain.value.to_ascii_lowercase()),
                3 => DomainPattern::Full(domain.value.to_ascii_lowercase()),
                kind => bail!("unsupported GeoSite domain type {kind}"),
            };
            patterns.push(pattern);
        }
        sites.insert(site.country_code.to_ascii_lowercase(), patterns);
    }
    Ok(sites)
}

fn decode_geoip(bytes: &[u8]) -> Result<HashMap<String, Vec<Ipv4Net>>> {
    let list = GeoIpList::decode(bytes).context("invalid V2Ray geoip.dat")?;
    let mut ips = HashMap::with_capacity(list.entry.len());
    for entry in list.entry {
        if entry.inverse_match {
            continue;
        }
        let networks = entry
            .cidr
            .into_iter()
            .filter(|cidr| cidr.ip.len() == 4 && cidr.prefix <= 32)
            .map(|cidr| {
                Ipv4Net::new(
                    Ipv4Addr::new(cidr.ip[0], cidr.ip[1], cidr.ip[2], cidr.ip[3]),
                    u8::try_from(cidr.prefix).expect("prefix was checked"),
                )
                .map(|network| network.trunc())
                .context("invalid IPv4 network in geoip.dat")
            })
            .collect::<Result<Vec<_>>>()?;
        ips.insert(entry.country_code.to_ascii_lowercase(), networks);
    }
    Ok(ips)
}

#[derive(Clone, PartialEq, Message)]
struct GeoSiteList {
    #[prost(message, repeated, tag = "1")]
    entry: Vec<GeoSite>,
}

#[derive(Clone, PartialEq, Message)]
struct GeoSite {
    #[prost(string, tag = "1")]
    country_code: String,
    #[prost(message, repeated, tag = "2")]
    domain: Vec<Domain>,
}

#[derive(Clone, PartialEq, Message)]
struct Domain {
    #[prost(enumeration = "DomainKind", tag = "1")]
    kind: i32,
    #[prost(string, tag = "2")]
    value: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, prost::Enumeration)]
enum DomainKind {
    Plain = 0,
    Regex = 1,
    RootDomain = 2,
    Full = 3,
}

#[derive(Clone, PartialEq, Message)]
struct GeoIpList {
    #[prost(message, repeated, tag = "1")]
    entry: Vec<GeoIp>,
}

#[derive(Clone, PartialEq, Message)]
struct GeoIp {
    #[prost(string, tag = "1")]
    country_code: String,
    #[prost(message, repeated, tag = "2")]
    cidr: Vec<Cidr>,
    #[prost(bool, tag = "3")]
    inverse_match: bool,
}

#[derive(Clone, PartialEq, Message)]
struct Cidr {
    #[prost(bytes = "vec", tag = "1")]
    ip: Vec<u8>,
    #[prost(uint32, tag = "2")]
    prefix: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_v2ray_geodata() {
        let sites = GeoSiteList {
            entry: vec![GeoSite {
                country_code: "CATEGORY-RU".into(),
                domain: vec![
                    Domain {
                        kind: DomainKind::RootDomain as i32,
                        value: "vk.com".into(),
                    },
                    Domain {
                        kind: DomainKind::Full as i32,
                        value: "example.ru".into(),
                    },
                    Domain {
                        kind: DomainKind::Regex as i32,
                        value: r"^\S+\.example\.com$".into(),
                    },
                ],
            }],
        }
        .encode_to_vec();
        let ips = GeoIpList {
            entry: vec![GeoIp {
                country_code: "RU".into(),
                cidr: vec![Cidr {
                    ip: vec![5, 136, 0, 0],
                    prefix: 13,
                }],
                inverse_match: false,
            }],
        }
        .encode_to_vec();

        let geodata = GeoData {
            sites: decode_geosite(&sites).unwrap(),
            ips: decode_geoip(&ips).unwrap(),
        };
        assert!(geodata.matches_site("category-ru", "m.vk.com"));
        assert!(geodata.matches_site("category-ru", "example.ru"));
        assert!(geodata.matches_site("category-ru", "api.example.com"));
        assert!(geodata.matches_ip("ru", "5.136.1.1".parse().unwrap()));
        assert!(!geodata.matches_ip("ru", "1.1.1.1".parse().unwrap()));
    }
}
