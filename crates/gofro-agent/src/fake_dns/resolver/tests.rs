use super::*;
use std::sync::Arc;

use hickory_proto::rr::Name;

use crate::{
    fake_dns::store::Store,
    geodata::GeoData,
    model::{DomainMatch, DomainRule, IpMatch, IpRule, RoutingConfig, RoutingMode},
};

fn test_lan() -> LanContext {
    LanContext {
        device: "br-home".into(),
        address: "100.64.0.1".parse().unwrap(),
        subnet: "100.64.0.0/24".parse().unwrap(),
    }
}

#[test]
fn overload_returns_servfail() {
    let mut request = Message::new();
    request.set_id(42).set_recursion_desired(true);

    let response = Message::from_vec(&failure_response(&request.to_vec().unwrap())).unwrap();

    assert_eq!(response.id(), 42);
    assert_eq!(response.response_code(), ResponseCode::ServFail);
}

#[test]
fn accepts_dns_service_labels() {
    assert_eq!(
        query_domain("_Minecraft._TCP.Example.com."),
        "_minecraft._tcp.example.com"
    );
}

#[test]
fn panel_hostname_never_needs_an_upstream() {
    let upstream = UdpSocket::bind("127.0.0.1:0").unwrap();
    upstream.set_nonblocking(true).unwrap();
    let dns = FakeDns {
        store: Mutex::new(
            Store::from_connection(rusqlite::Connection::open_in_memory().unwrap()).unwrap(),
        ),
        updates: RwLock::new(()),
        active: AtomicBool::new(false),
        vpn_enabled: AtomicBool::new(false),
    };
    for mode in [RoutingMode::Rules, RoutingMode::All] {
        let policy = RoutingPolicy::compile(
            RoutingConfig {
                domain_rules: vec![DomainRule {
                    name: "Block panel".into(),
                    enabled: true,
                    matcher: DomainMatch::Exact {
                        value: AP_DOMAIN.into(),
                    },
                    target: RouteTarget::Block,
                }],
                ip_rules: vec![],
                mode,
                ..RoutingConfig::default()
            },
            Arc::new(GeoData::default()),
        )
        .unwrap();
        for enabled in [false, true] {
            dns.set_vpn_enabled(enabled);
            for kind in [
                RecordType::A,
                RecordType::AAAA,
                RecordType::HTTPS,
                RecordType::SVCB,
                RecordType::TXT,
                RecordType::ANY,
            ] {
                let mut request = Message::new();
                request
                    .set_id(42)
                    .add_query(hickory_proto::op::Query::query(
                        Name::from_ascii("WiFi.GoFrO.NeT.").unwrap(),
                        kind,
                    ));
                let response = Message::from_vec(
                    &dns.process(
                        &request.to_vec().unwrap(),
                        &policy,
                        upstream.local_addr().unwrap(),
                        &test_lan(),
                    )
                    .unwrap(),
                )
                .unwrap();
                assert_eq!(response.id(), 42);
                assert_eq!(response.response_code(), ResponseCode::NoError);
                assert_eq!(response.queries(), request.queries());
                if kind == RecordType::A {
                    assert_eq!(response.answers().len(), 1);
                    assert_eq!(response.answers()[0].data(), &RData::A(A(PANEL_VIRTUAL_IP)));
                    assert_eq!(response.answers()[0].ttl(), 30);
                } else {
                    assert!(response.answers().is_empty());
                }
                assert_eq!(dns.count(), 0);
            }
        }
    }
    assert_eq!(
        upstream.recv(&mut [0; 512]).unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

#[test]
fn vpn_filter_removes_ipv6_and_https_records() {
    let name = Name::from_ascii("example.com.").unwrap();
    let mut records = vec![
        Record::from_rdata(name.clone(), 60, RData::A(A("1.1.1.1".parse().unwrap()))),
        Record::from_rdata(
            name,
            60,
            RData::AAAA(hickory_proto::rr::rdata::AAAA(
                "2001:db8::1".parse().unwrap(),
            )),
        ),
    ];

    assert!(retain_ipv4_records(&mut records));
    assert_eq!(records.len(), 1);
}

#[test]
fn vpn_filter_preserves_local_ipv6_answers() {
    let name = Name::from_ascii("router.home.").unwrap();
    let mut records = ["::1", "fd12::1", "fe80::1", "ff02::1"]
        .into_iter()
        .map(|ip| {
            Record::from_rdata(
                name.clone(),
                7,
                RData::AAAA(hickory_proto::rr::rdata::AAAA(ip.parse().unwrap())),
            )
        })
        .collect::<Vec<_>>();
    let original = records.clone();
    assert!(!retain_ipv4_records(&mut records));
    assert_eq!(records, original);
}

#[test]
fn discovered_subnet_answers_are_not_fake_or_ttl_clamped() {
    let policy = RoutingPolicy::compile(
        RoutingConfig {
            domain_rules: vec![],
            ip_rules: vec![],
            ..RoutingConfig::default()
        },
        Arc::new(GeoData::default()),
    )
    .unwrap();
    let name = Name::from_ascii("router.home.").unwrap();
    let mut records = ["100.64.0.1", "100.64.0.22", "127.0.0.1", "169.254.1.1"]
        .into_iter()
        .map(|ip| Record::from_rdata(name.clone(), 7, RData::A(A(ip.parse().unwrap()))))
        .collect::<Vec<_>>();
    let original = records.clone();
    let mut store =
        Store::from_connection(rusqlite::Connection::open_in_memory().unwrap()).unwrap();
    let mut added = Vec::new();
    let names = relevant_names(&records, "router.home");
    assert!(
        !rewrite_records(
            &mut records,
            "router.home",
            &policy,
            &names,
            &mut store,
            &mut added,
            &test_lan()
        )
        .unwrap()
    );
    assert_eq!(records, original);
    assert!(added.is_empty());
    assert_eq!(store.len(), 0);
}

#[test]
fn vpn_off_still_rewrites_block_domains_and_invalidates_dnssec() {
    let policy = RoutingPolicy::compile(
        RoutingConfig {
            domain_rules: vec![DomainRule {
                name: "Block".into(),
                enabled: true,
                matcher: DomainMatch::Exact {
                    value: "example.com".into(),
                },
                target: RouteTarget::Block,
            }],
            ip_rules: vec![],
            ..RoutingConfig::default()
        },
        Arc::new(GeoData::default()),
    )
    .unwrap();
    let mut store =
        Store::from_connection(rusqlite::Connection::open_in_memory().unwrap()).unwrap();
    // An installed lease avoids any external nft invocation in this test.
    let (mapping, _) = store
        .allocate(
            "example.com",
            "8.8.8.8".parse().unwrap(),
            RouteTarget::Block,
            60,
        )
        .unwrap();
    let dns = FakeDns {
        store: Mutex::new(store),
        updates: RwLock::new(()),
        active: AtomicBool::new(false),
        vpn_enabled: AtomicBool::new(false),
    };
    let name = Name::from_ascii("example.com.").unwrap();
    let mut message = Message::new();
    message
        .set_authentic_data(true)
        .add_answer(Record::from_rdata(
            name.clone(),
            1,
            RData::A(A(mapping.real)),
        ))
        .add_answer(Record::from_rdata(
            name.clone(),
            60,
            RData::AAAA(hickory_proto::rr::rdata::AAAA(
                "2001:db8::1".parse().unwrap(),
            )),
        ))
        .add_answer(Record::from_rdata(
            name,
            7,
            RData::AAAA(hickory_proto::rr::rdata::AAAA("fd12::1".parse().unwrap())),
        ));
    dns.rewrite_response(&mut message, "example.com", &policy, &test_lan())
        .unwrap();
    assert_eq!(message.answers().len(), 2);
    assert_eq!(message.answers()[0].data(), &RData::A(A(mapping.fake)));
    assert_eq!(message.answers()[0].ttl(), 30);
    assert_eq!(message.answers()[1].ttl(), 7);
    assert!(!message.authentic_data());
    assert_eq!(
        dataplane::effective_target(mapping.target, false),
        RouteTarget::Block
    );
}

#[test]
fn rewrites_mixed_answers_with_independent_targets() {
    let policy = RoutingPolicy::compile(
        RoutingConfig {
            domain_rules: vec![DomainRule {
                name: "Context".into(),
                enabled: true,
                matcher: DomainMatch::Exact {
                    value: "example.com".into(),
                },
                target: RouteTarget::Vpn,
            }],
            ip_rules: vec![IpRule {
                name: "Direct IP".into(),
                enabled: true,
                matcher: IpMatch::Cidr {
                    value: "1.1.1.0/24".into(),
                },
                target: RouteTarget::Direct,
            }],
            default_target: RouteTarget::Vpn,
            mode: RoutingMode::Rules,
            rule_order: Some(vec![
                crate::model::RuleRef::Ip { index: 0 },
                crate::model::RuleRef::Domain { index: 0 },
            ]),
        },
        Arc::new(GeoData::default()),
    )
    .unwrap();
    let name = Name::from_ascii("example.com.").unwrap();
    let mut records = vec![
        Record::from_rdata(name.clone(), 60, RData::A(A("1.1.1.1".parse().unwrap()))),
        Record::from_rdata(name, 60, RData::A(A("8.8.8.8".parse().unwrap()))),
    ];
    let mut store =
        Store::from_connection(rusqlite::Connection::open_in_memory().unwrap()).unwrap();
    let mut added = vec![];
    let names = relevant_names(&records, "example.com");
    rewrite_records(
        &mut records,
        "example.com",
        &policy,
        &names,
        &mut store,
        &mut added,
        &test_lan(),
    )
    .unwrap();
    assert_eq!(
        added
            .iter()
            .map(|mapping| mapping.target)
            .collect::<Vec<_>>(),
        vec![RouteTarget::Direct, RouteTarget::Vpn]
    );
}

#[test]
fn vpn_off_dns_leases_recover_policy_intent_on_full_reclassification() {
    let policy = RoutingPolicy::compile(
        RoutingConfig {
            domain_rules: vec![DomainRule {
                name: "VPN domain".into(),
                enabled: true,
                matcher: DomainMatch::Exact {
                    value: "example.com".into(),
                },
                target: RouteTarget::Vpn,
            }],
            ip_rules: vec![],
            ..RoutingConfig::default()
        },
        Arc::new(GeoData::default()),
    )
    .unwrap();
    let mut store =
        Store::from_connection(rusqlite::Connection::open_in_memory().unwrap()).unwrap();
    let mut records = vec![Record::from_rdata(
        Name::from_ascii("example.com.").unwrap(),
        60,
        RData::A(A("8.8.8.8".parse().unwrap())),
    )];
    let names = relevant_names(&records, "example.com");
    let mut added = Vec::new();
    rewrite_records(
        &mut records,
        "example.com",
        &policy,
        &names,
        &mut store,
        &mut added,
        &test_lan(),
    )
    .unwrap();
    let dns = FakeDns {
        store: Mutex::new(store),
        updates: RwLock::new(()),
        active: AtomicBool::new(false),
        vpn_enabled: AtomicBool::new(false),
    };
    assert_eq!(added[0].target, RouteTarget::Vpn);
    dns.commit_targets(&policy).unwrap();
    assert_eq!(dns.reclassified(&policy).unwrap(), added);
    dns.set_vpn_enabled(true);
    assert_eq!(dns.reclassified(&policy).unwrap(), added);
    let mut direct = policy.config().clone();
    direct.domain_rules[0].target = RouteTarget::Direct;
    let direct = RoutingPolicy::compile(direct, Arc::new(GeoData::default())).unwrap();
    dns.commit_targets(&direct).unwrap();
    // Full installs derive from domain+real IP, not a cached effective target.
    assert_eq!(dns.reclassified(&policy).unwrap(), added);
}

#[test]
fn block_domains_keep_lan_answers_direct_with_legacy_and_explicit_order() {
    let mut config = RoutingConfig {
        domain_rules: vec![DomainRule {
            name: "Block domain".into(),
            enabled: true,
            matcher: DomainMatch::Exact {
                value: "example.com".into(),
            },
            target: RouteTarget::Block,
        }],
        ip_rules: vec![],
        default_target: RouteTarget::Vpn,
        mode: RoutingMode::Rules,
        rule_order: None,
    };
    for order in [None, Some(vec![crate::model::RuleRef::Domain { index: 0 }])] {
        config.rule_order = order;
        let policy = RoutingPolicy::compile(config.clone(), Arc::new(GeoData::default())).unwrap();
        let name = Name::from_ascii("example.com.").unwrap();
        let mut records = vec![
            Record::from_rdata(
                name.clone(),
                60,
                RData::A(A("192.168.1.1".parse().unwrap())),
            ),
            Record::from_rdata(name, 60, RData::A(A("8.8.8.8".parse().unwrap()))),
        ];
        let mut store =
            Store::from_connection(rusqlite::Connection::open_in_memory().unwrap()).unwrap();
        let mut added = vec![];
        let names = relevant_names(&records, "example.com");
        rewrite_records(
            &mut records,
            "example.com",
            &policy,
            &names,
            &mut store,
            &mut added,
            &test_lan(),
        )
        .unwrap();
        assert_eq!(
            added
                .iter()
                .map(|mapping| mapping.target)
                .collect::<Vec<_>>(),
            vec![RouteTarget::Block]
        );
        assert_eq!(
            records[0].data(),
            &RData::A(A("192.168.1.1".parse().unwrap()))
        );
    }
}
