use std::{
    fs,
    sync::atomic::{AtomicU64, Ordering},
};

use prost::Message;

use super::*;
use crate::{
    model::{DomainRule, IpRule, RouteTarget},
    routing::RoutingPolicy,
};

#[derive(Clone, PartialEq, Message)]
struct SiteList {
    #[prost(message, repeated, tag = "1")]
    entry: Vec<SiteRecord>,
}
#[derive(Clone, PartialEq, Message)]
struct SiteRecord {
    #[prost(string, tag = "1")]
    country_code: String,
    #[prost(message, repeated, tag = "2")]
    domain: Vec<Domain>,
}
#[derive(Clone, PartialEq, Message)]
struct Domain {
    #[prost(int32, tag = "1")]
    kind: i32,
    #[prost(string, tag = "2")]
    value: String,
}
#[derive(Clone, PartialEq, Message)]
struct IpList {
    #[prost(message, repeated, tag = "1")]
    entry: Vec<IpRecord>,
}
#[derive(Clone, PartialEq, Message)]
struct IpRecord {
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

struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new() -> Self {
        static ID: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "gofro-geodata-{}-{}",
            std::process::id(),
            ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        fs::write(
            root.join("sites"),
            SiteList {
                entry: vec![
                    SiteRecord {
                        country_code: "CATEGORY-RU".into(),
                        domain: vec![
                            Domain {
                                kind: 0,
                                value: "needle".into(),
                            },
                            Domain {
                                kind: 1,
                                value: r"^api\d+\.example\.com$".into(),
                            },
                            Domain {
                                kind: 2,
                                value: "VK.COM".into(),
                            },
                            Domain {
                                kind: 3,
                                value: "only.example".into(),
                            },
                        ],
                    },
                    // Unused malformed regex must never be compiled at boot.
                    SiteRecord {
                        country_code: "OTHER".into(),
                        domain: vec![Domain {
                            kind: 1,
                            value: "[".into(),
                        }],
                    },
                ],
            }
            .encode_to_vec(),
        )
        .unwrap();
        fs::write(
            root.join("ips"),
            IpList {
                entry: vec![
                    IpRecord {
                        country_code: "RU".into(),
                        cidr: vec![
                            Cidr {
                                ip: vec![5, 136, 0, 0],
                                prefix: 14,
                            },
                            Cidr {
                                ip: vec![5, 140, 0, 0],
                                prefix: 14,
                            },
                            Cidr {
                                ip: vec![5, 136, 1, 0],
                                prefix: 24,
                            },
                            Cidr {
                                ip: vec![0; 16],
                                prefix: 32,
                            },
                            Cidr {
                                ip: vec![1, 1, 1, 1],
                                prefix: 33,
                            },
                        ],
                        inverse_match: false,
                    },
                    IpRecord {
                        country_code: "INVERSE".into(),
                        cidr: vec![],
                        inverse_match: true,
                    },
                ],
            }
            .encode_to_vec(),
        )
        .unwrap();
        Self(root)
    }
    fn load(&self) -> Arc<GeoData> {
        Arc::new(GeoData::load(&self.0.join("sites"), &self.0.join("ips")).unwrap())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn routing() -> RoutingConfig {
    RoutingConfig {
        domain_rules: vec![DomainRule {
            name: "RU".into(),
            enabled: true,
            matcher: DomainMatch::GeoSite {
                value: "category-ru".into(),
            },
            target: RouteTarget::Direct,
        }],
        ip_rules: vec![IpRule {
            name: "RU".into(),
            enabled: true,
            matcher: IpMatch::GeoIp { value: "ru".into() },
            target: RouteTarget::Direct,
        }],
        ..RoutingConfig::default()
    }
}

#[test]
fn selected_tags_preserve_domain_semantics_and_aggregate_ipv4() {
    let fixture = Fixture::new();
    let catalog = fixture.load();
    assert!(catalog.has_site("other"));
    assert!(!catalog.has_ip("inverse"));
    assert!(catalog.sites.is_empty() && catalog.ips.is_empty());
    let data = catalog.select(&routing()).unwrap();
    for domain in [
        "vk.com",
        "m.vk.com",
        "only.example",
        "api12.example.com",
        "some-needle.example",
    ] {
        assert!(data.matches_site("category-ru", domain), "{domain}");
    }
    for domain in [
        "evk.com",
        "m.only.example",
        "api.example.com",
        "example.org",
    ] {
        assert!(!data.matches_site("category-ru", domain), "{domain}");
    }
    assert_eq!(
        data.ip_networks("ru"),
        ["5.136.0.0/13".parse::<Ipv4Net>().unwrap()]
    );
    assert!(data.matches_ip("ru", "5.143.255.255".parse().unwrap()));
    assert!(!data.matches_ip("ru", "5.144.0.0".parse().unwrap()));
    assert!(RoutingPolicy::compile(routing(), catalog).is_ok());
}

#[test]
fn ipv4_aggregation_never_expands_the_top_of_the_address_space() {
    for (input, expected) in [
        (vec!["255.255.255.254/32"], vec!["255.255.255.254/32"]),
        (
            vec!["255.255.255.255/32", "255.255.255.254/32"],
            vec!["255.255.255.254/31"],
        ),
        (vec!["255.255.255.255/32", "0.0.0.0/0"], vec!["0.0.0.0/0"]),
        (
            vec![
                "10.0.3.0/24",
                "10.0.0.0/24",
                "10.0.1.0/24",
                "10.0.2.0/24",
                "10.0.1.128/25",
            ],
            vec!["10.0.0.0/22"],
        ),
    ] {
        let mut networks: Vec<_> = input
            .into_iter()
            .map(|value| value.parse().unwrap())
            .collect();
        aggregate_ipv4(&mut networks);
        let expected: Vec<Ipv4Net> = expected
            .into_iter()
            .map(|value| value.parse().unwrap())
            .collect();
        assert_eq!(networks, expected);
    }
}

#[test]
fn updates_share_active_tags_and_release_disabled_and_all_mode_data() {
    let fixture = Fixture::new();
    let catalog = fixture.load();
    let first = catalog.select(&routing()).unwrap();
    let second = catalog.select(&routing()).unwrap();
    assert!(Arc::ptr_eq(
        &first.sites["category-ru"],
        &second.sites["category-ru"]
    ));
    assert!(Arc::ptr_eq(&first.ips["ru"], &second.ips["ru"]));
    let weak = Arc::downgrade(&first.sites["category-ru"]);
    let mut disabled = routing();
    disabled.domain_rules[0].enabled = false;
    assert!(catalog.select(&disabled).unwrap().sites.is_empty());
    let all = RoutingConfig {
        mode: RoutingMode::All,
        ..routing()
    };
    let all = catalog.select(&all).unwrap();
    assert!(all.sites.is_empty() && all.ips.is_empty());
    drop(first);
    drop(second);
    assert!(weak.upgrade().is_none());
    let mut other = routing();
    other.domain_rules[0].matcher = DomainMatch::GeoSite {
        value: "other".into(),
    };
    assert!(catalog.select(&other).is_err());
}

#[test]
fn indexing_rejects_truncation_and_reads_reordered_unknown_fields() {
    let fixture = Fixture::new();
    let path = fixture.0.join("sites");
    // Country code after the domain, with an unknown fixed-width field.
    let mut entry = Vec::new();
    prost::encoding::encode_key(2, prost::encoding::WireType::LengthDelimited, &mut entry);
    let domain = Domain {
        kind: 2,
        value: "example.com".into(),
    }
    .encode_to_vec();
    prost::encoding::encode_varint(domain.len() as u64, &mut entry);
    entry.extend(domain);
    entry.extend_from_slice(&[0x29, 0, 0, 0, 0, 0, 0, 0, 0]);
    entry.extend_from_slice(&[0x0a, 2, b'R', b'U']);
    let mut bytes = vec![0x0a];
    prost::encoding::encode_varint(entry.len() as u64, &mut bytes);
    bytes.extend(entry);
    fs::write(&path, &bytes).unwrap();
    assert!(fixture.load().has_site("ru"));
    bytes.pop();
    fs::write(&path, bytes).unwrap();
    assert!(GeoData::load(&path, &fixture.0.join("ips")).is_err());
}

#[test]
fn open_catalog_pins_data_across_release_file_replacement() {
    let fixture = Fixture::new();
    let catalog = fixture.load();
    fs::rename(fixture.0.join("sites"), fixture.0.join("old-sites")).unwrap();
    fs::write(fixture.0.join("sites"), []).unwrap();
    assert!(
        catalog
            .select(&routing())
            .unwrap()
            .matches_site("category-ru", "vk.com")
    );
}

#[test]
fn compact_policy_admission_rejects_oversized_data_without_losing_active_tags() {
    let fixture = Fixture::new();
    for limits in [
        ResourceLimits {
            geosite_bytes: 1,
            ..ResourceLimits::COMPACT
        },
        ResourceLimits {
            regex_count: 0,
            ..ResourceLimits::COMPACT
        },
        ResourceLimits {
            geoip_intervals: 0,
            ..ResourceLimits::COMPACT
        },
    ] {
        let catalog = Arc::new(
            GeoData::load_with_limits(&fixture.0.join("sites"), &fixture.0.join("ips"), limits)
                .unwrap(),
        );
        assert!(catalog.select(&routing()).is_err());
        let all = RoutingConfig {
            mode: RoutingMode::All,
            ..routing()
        };
        assert!(catalog.select(&all).is_ok());
    }
    let catalog = Arc::new(
        GeoData::load_with_limits(
            &fixture.0.join("sites"),
            &fixture.0.join("ips"),
            ResourceLimits::COMPACT,
        )
        .unwrap(),
    );
    let active = catalog.select(&routing()).unwrap();
    let mut next = routing();
    next.domain_rules[0].matcher = DomainMatch::GeoSite {
        value: "other".into(),
    };
    assert!(catalog.select(&next).is_err());
    assert!(active.matches_site("category-ru", "vk.com"));
}

#[test]
#[ignore = "requires GEOSITE_DAT, GEOIP_DAT and COMPACT_GEOIP_DAT release databases"]
fn real_catalog_matches_independent_protobuf_reference() {
    let site_path = std::env::var("GEOSITE_DAT").unwrap();
    let ip_path = std::env::var("GEOIP_DAT").unwrap();
    let compact_path = std::env::var("COMPACT_GEOIP_DAT").unwrap();
    let catalog = Arc::new(GeoData::load(Path::new(&site_path), Path::new(&compact_path)).unwrap());
    let ips = IpList::decode(fs::read(ip_path).unwrap().as_slice()).unwrap();
    let mut ip_tags = 0;
    for entry in ips.entry {
        let tag = entry.country_code.to_ascii_lowercase();
        if entry.inverse_match {
            assert!(!catalog.has_ip(&tag));
            continue;
        }
        let expected: Vec<_> = entry
            .cidr
            .into_iter()
            .filter(|cidr| cidr.ip.len() == 4 && cidr.prefix <= 32)
            .map(|cidr| {
                Ipv4Net::new(
                    Ipv4Addr::new(cidr.ip[0], cidr.ip[1], cidr.ip[2], cidr.ip[3]),
                    cidr.prefix as u8,
                )
                .unwrap()
                .trunc()
            })
            .collect();
        let config = RoutingConfig {
            domain_rules: vec![],
            ip_rules: vec![IpRule {
                name: tag.clone(),
                enabled: true,
                target: RouteTarget::Direct,
                matcher: IpMatch::GeoIp { value: tag.clone() },
            }],
            ..RoutingConfig::default()
        };
        let selected = catalog.select(&config).unwrap();
        assert_eq!(
            selected.ip_networks(&tag),
            Ipv4Net::aggregate(&expected),
            "{tag}"
        );
        ip_tags += 1;
    }

    let sites = SiteList::decode(fs::read(site_path).unwrap().as_slice()).unwrap();
    let mut site_tags = 0;
    for entry in sites.entry {
        let tag = entry.country_code.to_ascii_lowercase();
        let config = RoutingConfig {
            domain_rules: vec![DomainRule {
                name: tag.clone(),
                enabled: true,
                target: RouteTarget::Direct,
                matcher: DomainMatch::GeoSite { value: tag.clone() },
            }],
            ip_rules: vec![],
            ..RoutingConfig::default()
        };
        let selected = catalog.select(&config).unwrap();
        let site = &selected.sites[&tag];
        assert_eq!(
            site.full
                .iter()
                .map(|value| value.as_ref())
                .collect::<HashSet<_>>(),
            entry
                .domain
                .iter()
                .filter(|domain| domain.kind == 3)
                .map(|domain| domain.value.as_str())
                .collect(),
            "{tag}"
        );
        assert_eq!(
            site.roots
                .iter()
                .map(|value| value.as_ref())
                .collect::<HashSet<_>>(),
            entry
                .domain
                .iter()
                .filter(|domain| domain.kind == 2)
                .map(|domain| domain.value.as_str())
                .collect(),
            "{tag}"
        );
        assert_eq!(
            site.keywords
                .iter()
                .map(|value| value.as_ref())
                .collect::<Vec<_>>(),
            entry
                .domain
                .iter()
                .filter(|domain| domain.kind == 0)
                .map(|domain| domain.value.as_str())
                .collect::<Vec<_>>(),
            "{tag}"
        );
        assert_eq!(
            site.regexes.iter().map(Regex::as_str).collect::<Vec<_>>(),
            entry
                .domain
                .iter()
                .filter(|domain| domain.kind == 1)
                .map(|domain| domain.value.as_str())
                .collect::<Vec<_>>(),
            "{tag}"
        );
        site_tags += 1;
    }
    println!(
        "Verified complete catalogs against prost: {site_tags} GeoSite tags, {ip_tags} GeoIP tags"
    );
}
