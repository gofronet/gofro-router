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

fn dns_policy(target: RouteTarget, fallback: RouteTarget, mode: RoutingMode) -> RoutingPolicy {
    RoutingPolicy::compile(
        RoutingConfig {
            domain_rules: vec![DomainRule {
                name: "DNS path".into(),
                enabled: true,
                matcher: DomainMatch::Suffix {
                    value: "routed.example".into(),
                },
                target,
            }],
            ip_rules: vec![],
            default_target: fallback,
            mode,
            rule_order: None,
        },
        Arc::new(GeoData::default()),
    )
    .unwrap()
}

#[test]
fn dns_egress_uses_separate_direct_vpn_and_warp_paths_without_direct_fallback() {
    let local = "100.64.0.1:53".parse().unwrap();
    let public = "1.1.1.1:53".parse().unwrap();
    for target in [
        RouteTarget::Direct,
        RouteTarget::Vpn,
        RouteTarget::Warp,
        RouteTarget::Block,
    ] {
        for enabled in [false, true] {
            let policy = dns_policy(target, RouteTarget::Direct, RoutingMode::Rules);
            let actual = upstream_route("sub.routed.example", &policy, local, enabled, &test_lan());
            let effective = dataplane::effective_target(target, enabled);
            let expected = match effective {
                RouteTarget::Direct => Some(UpstreamRoute {
                    address: local,
                    mark: 0x10000,
                }),
                RouteTarget::Vpn => Some(UpstreamRoute {
                    address: public,
                    mark: 0x20000,
                }),
                RouteTarget::Warp => Some(UpstreamRoute {
                    address: "1.0.0.1:53".parse().unwrap(),
                    mark: 0x60000,
                }),
                RouteTarget::Block => None,
            };
            assert_eq!(actual, expected, "{target:?}, VPN enabled={enabled}");
        }
    }
}

#[test]
fn dns_egress_follows_fallback_and_all_mode_without_an_isp_probe() {
    let local = "100.64.0.1:53".parse().unwrap();
    for fallback in [
        RouteTarget::Direct,
        RouteTarget::Vpn,
        RouteTarget::Warp,
        RouteTarget::Block,
    ] {
        for mode in [RoutingMode::Rules, RoutingMode::All] {
            let policy = dns_policy(RouteTarget::Direct, fallback, mode);
            let actual = upstream_route("unmatched.example", &policy, local, true, &test_lan());
            let target = if mode == RoutingMode::All {
                RouteTarget::Vpn
            } else {
                fallback
            };
            if target == RouteTarget::Block {
                assert!(actual.is_none());
            } else {
                let route = actual.unwrap();
                assert_eq!(route.mark, dataplane::target_mark(target));
                assert_eq!(route.address == local, target == RouteTarget::Direct);
            }
        }
    }
}

#[test]
fn vpn_and_warp_dns_endpoints_cannot_share_an_ephemeral_conntrack_tuple() {
    let policy = dns_policy(RouteTarget::Warp, RouteTarget::Vpn, RoutingMode::Rules);
    let local = "100.64.0.1:53".parse().unwrap();
    let vpn = upstream_route("fallback.example", &policy, local, true, &test_lan()).unwrap();
    let warp = upstream_route("routed.example", &policy, local, true, &test_lan()).unwrap();
    assert_ne!(vpn.address, warp.address);
    assert_ne!(vpn.mark, warp.mark);
}

#[test]
fn local_hostnames_and_private_reverse_zones_never_leave_the_local_resolver() {
    let policy = dns_policy(RouteTarget::Warp, RouteTarget::Warp, RoutingMode::Rules);
    let local = "100.64.0.1:53".parse().unwrap();
    for name in [
        "printer",
        "localhost",
        "host.localhost",
        "nas.homelab.lan",
        "_services._dns-sd._udp.local",
        "router.home.arpa",
        "nas.internal",
        "1.0.64.100.in-addr.arpa",
        "1.1.168.192.in-addr.arpa",
        "168.192.in-addr.arpa",
        "c.f.ip6.arpa",
        "0.8.e.f.ip6.arpa",
    ] {
        assert!(local_dns_name(name, &test_lan()), "{name}");
        assert_eq!(
            upstream_route(name, &policy, local, true, &test_lan()),
            Some(UpstreamRoute {
                address: local,
                mark: 0x10000
            })
        );
    }
    for name in [
        "example.com",
        "notlocal.com",
        "printer.lan.evil",
        "1.1.1.1.in-addr.arpa",
        "8.8.8.8.in-addr.arpa",
        "8.8.8.8.ip6.arpa",
    ] {
        assert!(!local_dns_name(name, &test_lan()), "{name}");
        assert_eq!(
            upstream_route(name, &policy, local, true, &test_lan())
                .unwrap()
                .mark,
            0x60000
        );
    }
}

#[test]
fn block_and_disabled_warp_dns_are_refused_without_contacting_an_upstream() {
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
    for (target, enabled) in [(RouteTarget::Block, true), (RouteTarget::Warp, false)] {
        let policy = dns_policy(target, RouteTarget::Vpn, RoutingMode::Rules);
        dns.set_vpn_enabled(enabled);
        for kind in [
            RecordType::A,
            RecordType::AAAA,
            RecordType::HTTPS,
            RecordType::TXT,
        ] {
            let mut request = Message::new();
            request
                .set_id(29)
                .add_query(hickory_proto::op::Query::query(
                    Name::from_ascii("Sub.Routed.Example.").unwrap(),
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
            assert_eq!(response.id(), 29);
            assert_eq!(response.queries(), request.queries());
            assert_eq!(response.response_code(), ResponseCode::Refused);
            assert!(response.answers().is_empty());
        }
    }
    assert_eq!(
        upstream.recv(&mut [0; 512]).unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

#[test]
fn protected_queries_remove_client_subnets_but_keep_dnssec_and_other_edns_options() {
    use hickory_proto::rr::rdata::opt::EdnsOption;
    let mut request = Message::new();
    request
        .set_id(29)
        .set_recursion_desired(true)
        .add_query(hickory_proto::op::Query::query(
            Name::from_ascii("routed.example.").unwrap(),
            RecordType::A,
        ));
    let edns = request.extensions_mut().get_or_insert_default();
    edns.set_dnssec_ok(true).set_max_payload(1232);
    edns.options_mut()
        .insert(EdnsOption::Subnet("203.0.113.0/24".parse().unwrap()));
    edns.options_mut()
        .insert(EdnsOption::Subnet("2001:db8::/48".parse().unwrap()));
    edns.options_mut()
        .insert(EdnsOption::Unknown(65001, vec![1, 2, 3]));
    let original = request.to_vec().unwrap();
    for target in [RouteTarget::Vpn, RouteTarget::Warp] {
        let route = UpstreamRoute {
            address: TUNNEL_DNS_UPSTREAM,
            mark: dataplane::target_mark(target),
        };
        let outgoing = upstream_packet(&original, &request, route).unwrap();
        assert!(matches!(outgoing, Cow::Owned(_)));
        let decoded = Message::from_vec(&outgoing).unwrap();
        assert_eq!(decoded.id(), request.id());
        assert_eq!(decoded.queries(), request.queries());
        assert_eq!(decoded.recursion_desired(), request.recursion_desired());
        let edns = decoded.extensions().as_ref().unwrap();
        assert!(edns.flags().dnssec_ok);
        assert_eq!(edns.max_payload(), 1232);
        assert!(edns.options().get(EdnsCode::Subnet).is_none());
        assert_eq!(
            edns.options().get(EdnsCode::Unknown(65001)),
            Some(&EdnsOption::Unknown(65001, vec![1, 2, 3]))
        );
    }
    let direct = UpstreamRoute {
        address: "127.0.0.1:53".parse().unwrap(),
        mark: 0x10000,
    };
    assert!(matches!(
        upstream_packet(&original, &request, direct).unwrap(),
        Cow::Borrowed(_)
    ));
    assert_eq!(
        request.to_vec().unwrap(),
        original,
        "do not mutate the original request"
    );
    request
        .extensions_mut()
        .as_mut()
        .unwrap()
        .options_mut()
        .remove(EdnsCode::Subnet);
    let without_subnet = request.to_vec().unwrap();
    let protected = UpstreamRoute {
        address: TUNNEL_DNS_UPSTREAM,
        mark: 0x60000,
    };
    assert!(matches!(
        upstream_packet(&without_subnet, &request, protected).unwrap(),
        Cow::Borrowed(_)
    ));
}

#[test]
fn ambiguous_question_sets_cannot_disclose_protected_names_through_a_direct_first_question() {
    let upstream = UdpSocket::bind("127.0.0.1:0").unwrap();
    upstream.set_nonblocking(true).unwrap();
    let dns = FakeDns {
        store: Mutex::new(
            Store::from_connection(rusqlite::Connection::open_in_memory().unwrap()).unwrap(),
        ),
        updates: RwLock::new(()),
        active: AtomicBool::new(false),
        vpn_enabled: AtomicBool::new(true),
    };
    let policy = dns_policy(RouteTarget::Warp, RouteTarget::Direct, RoutingMode::Rules);
    for names in [vec![], vec!["direct.example.", "routed.example."]] {
        let mut request = Message::new();
        request.set_id(29003);
        for name in names {
            request.add_query(hickory_proto::op::Query::query(
                Name::from_ascii(name).unwrap(),
                RecordType::A,
            ));
        }
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
        assert_eq!(response.id(), request.id());
        assert_eq!(response.queries(), request.queries());
        assert_eq!(response.response_code(), ResponseCode::FormErr);
    }
    assert_eq!(
        upstream.recv(&mut [0; 512]).unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
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
fn small_udp_buffer_recovers_large_and_tc_responses_over_tcp() {
    use hickory_proto::{op::Query, rr::rdata::TXT};
    use std::{net::TcpListener, thread, time::Instant};

    for truncated in [false, true] {
        let tcp = TcpListener::bind("127.0.0.1:0").unwrap();
        tcp.set_nonblocking(true).unwrap();
        let address = tcp.local_addr().unwrap();
        let udp = UdpSocket::bind(address).unwrap();
        udp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut question = Message::new();
        question.set_id(42).add_query(Query::query(
            Name::from_ascii("large.example").unwrap(),
            RecordType::TXT,
        ));
        let request = question.to_vec().unwrap();
        let mut answer = question.clone();
        answer.set_message_type(MessageType::Response);
        for _ in 0..32 {
            answer.add_answer(Record::from_rdata(
                Name::from_ascii("large.example").unwrap(),
                30,
                RData::TXT(TXT::new(vec!["x".repeat(240)])),
            ));
        }
        let response = answer.to_vec().unwrap();
        assert!(response.len() > UDP_RESPONSE_BUFFER);
        let expected = response.clone();
        let expected_request = request.clone();
        let server = thread::spawn(move || {
            let mut buffer = [0; 2048];
            let (length, peer) = udp.recv_from(&mut buffer).unwrap();
            assert_eq!(&buffer[..length], expected_request.as_slice());
            if truncated {
                question
                    .set_message_type(MessageType::Response)
                    .set_truncated(true);
                udp.send_to(&question.to_vec().unwrap(), peer).unwrap();
            } else {
                udp.send_to(&response, peer).unwrap();
            }
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match tcp.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(error) => panic!("TCP fallback was not attempted: {error}"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut length = [0; 2];
            stream.read_exact(&mut length).unwrap();
            let mut body = vec![0; usize::from(u16::from_be_bytes(length))];
            stream.read_exact(&mut body).unwrap();
            assert_eq!(body, expected_request);
            stream
                .write_all(&(response.len() as u16).to_be_bytes())
                .unwrap();
            stream.write_all(&response).unwrap();
        });
        assert_eq!(
            query_upstream_with_mark(&request, address, 0).unwrap(),
            expected
        );
        server.join().unwrap();
    }
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
fn vpn_off_still_rewrites_block_and_warp_domains_and_invalidates_dnssec() {
    for target in [RouteTarget::Block, RouteTarget::Warp] {
        let policy = RoutingPolicy::compile(
            RoutingConfig {
                domain_rules: vec![DomainRule {
                    name: "Block".into(),
                    enabled: true,
                    matcher: DomainMatch::Exact {
                        value: "example.com".into(),
                    },
                    target,
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
            .allocate("example.com", "8.8.8.8".parse().unwrap(), target, 60)
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
