use std::{env, fs, sync::Arc};

use super::*;
use crate::{
    geodata::GeoData,
    model::{IpMatch, IpRule, RoutingConfig, RoutingMode, RuleRef},
};

const PANEL_PORTS: PanelPorts = PanelPorts {
    http: 8081,
    https: 8443,
};

#[test]
fn full_exclusions_precede_all_policy_and_dns_redirect_but_preserve_dnat_and_vip_safety() {
    let lan = LanContext {
        device: "br-home".into(),
        address: "192.168.4.1".parse().unwrap(),
        subnet: "192.168.4.0/24".parse().unwrap(),
    };
    let mac = "02:ab:cd:ef:00:01".parse().unwrap();
    for mode in [RoutingMode::All, RoutingMode::Rules] {
        for enabled in [false, true] {
            let policy = RoutingPolicy::compile(
                RoutingConfig {
                    domain_rules: vec![],
                    ip_rules: vec![],
                    default_target: RouteTarget::Block,
                    mode,
                    rule_order: None,
                },
                Arc::new(GeoData::default()),
            )
            .unwrap();
            let mapping = FakeMapping {
                fake: "198.18.0.1".parse().unwrap(),
                real: "1.1.1.1".parse().unwrap(),
                target: RouteTarget::Block,
            };
            let script = render_with_exclusions(
                &lan,
                5353,
                enabled,
                &policy,
                &[mapping],
                PANEL_PORTS,
                &[mac],
            );
            let bypass = script
                .find("ether saddr @device_exclusions meta mark set")
                .unwrap();
            assert!(
                script
                    .find("ip daddr 198.18.0.0 meta l4proto != tcp drop")
                    .unwrap()
                    < bypass
            );
            for protocol in ["udp", "tcp"] {
                assert!(script.find(&format!("ct direction original {protocol} dport 53 ct mark set ct mark | 0x40000000")).unwrap() < bypass);
            }
            assert!(script.contains("ether saddr @device_exclusions meta mark set (meta mark & 0xfffcffff) | 65536 ct mark set (ct mark & 0xfffcffff) | 65536 return"));
            assert!(bypass < script.find("ip daddr @fake_block").unwrap());
            assert!(script.contains("fake_to_real { 198.18.0.1 : 1.1.1.1 }"));
            assert!(script.contains("dnat ip to ip daddr map @fake_to_real"));
            assert!(
                script
                    .find("gofro_dns iifname \"br-home\" ether saddr @device_exclusions return")
                    .unwrap()
                    < script.find("udp dport 53 redirect").unwrap()
            );
            assert_eq!(
                script.contains("ether saddr != @device_exclusions meta nfproto ipv6 drop"),
                enabled
            );
            assert!(
                script
                    .contains("add set inet gofro_routing device_exclusions { type ether_addr; }")
            );
            assert!(script.contains(
                "add element inet gofro_routing device_exclusions { 02:ab:cd:ef:00:01 }"
            ));
        }
    }
}

#[test]
fn cold_guard_and_atomic_publication_cover_add_remove_and_legacy_tables() {
    let lan = LanContext {
        device: "br-home".into(),
        address: "192.168.4.1".parse().unwrap(),
        subnet: "192.168.4.0/24".parse().unwrap(),
    };
    let mac = "02:ab:cd:ef:00:01".parse().unwrap();
    let empty = render_guard_with_exclusions(&lan, &[]);
    let chain = empty
        .lines()
        .filter(|line| line.contains("add chain") || line.contains("add rule"))
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(
        chain,
        "add chain inet gofro_guard gofro_guard { type filter hook forward priority filter; policy accept; }\nadd rule inet gofro_guard gofro_guard iifname \"br-home\" oifname != \"br-home\" drop"
    );
    let cold = render_startup(&lan, &[mac], false, false).unwrap();
    assert!(cold.contains("oifname != \"br-home\" ether saddr != @device_exclusions drop"));
    assert!(!cold.contains("gofro_routing"));
    assert!(render_startup(&lan, &[mac], true, false).is_err());
    assert!(render_startup(&lan, &[], true, false).is_ok());
    for exclusions in [vec![mac], vec![]] {
        let publish = render_publish_exclusions(&lan, &exclusions);
        assert_eq!(
            render_startup(&lan, &exclusions, true, true).unwrap(),
            publish
        );
        assert!(publish.starts_with("flush set inet gofro_routing device_exclusions\n"));
        assert!(publish.contains("destroy table inet gofro_guard\n"));
        assert_eq!(
            publish.contains("ether saddr != @device_exclusions"),
            !exclusions.is_empty()
        );
    }
}

#[test]
fn startup_does_not_mistake_a_placeholder_set_for_an_upgraded_table() {
    use serde_json::json;
    let mut entries = vec![json!({"set": {"name": "device_exclusions", "type": "ether_addr"}})];
    assert!(!has_exclusion_return(&entries, "gofro_mark"));
    entries.push(json!({"rule": {"chain": "gofro_mark", "expr": [
        {"match": {"op": "==", "left": {"payload": {"protocol": "ether", "field": "saddr"}}, "right": "@device_exclusions"}},
        {"return": null}
    ]}}));
    assert!(has_exclusion_return(&entries, "gofro_mark"));
    assert!(!has_exclusion_return(&entries, "gofro_dns"));
}

#[test]
fn panel_vip_is_direct_and_dnat_is_lan_only_with_unsupported_traffic_dropped() {
    for device in ["br-home", "lan0"] {
        let lan = LanContext {
            device: device.into(),
            address: "100.64.0.1".parse().unwrap(),
            subnet: "100.64.0.0/24".parse().unwrap(),
        };
        for mode in [RoutingMode::Rules, RoutingMode::All] {
            let policy = RoutingPolicy::compile(
                RoutingConfig {
                    domain_rules: vec![],
                    ip_rules: vec![IpRule {
                        name: "Block everything".into(),
                        enabled: true,
                        matcher: IpMatch::Cidr {
                            value: "0.0.0.0/0".into(),
                        },
                        target: RouteTarget::Block,
                    }],
                    default_target: RouteTarget::Block,
                    mode,
                    rule_order: None,
                },
                Arc::new(GeoData::default()),
            )
            .unwrap();
            for enabled in [false, true] {
                for (ports, http_sources, https_sources, allowed) in [
                    (PANEL_PORTS, "80, 8081", "443, 8443", "80, 443, 8081, 8443"),
                    (
                        PanelPorts {
                            http: 80,
                            https: 443,
                        },
                        "80, 8081",
                        "443, 8443",
                        "80, 443, 8081, 8443",
                    ),
                    (
                        PanelPorts {
                            http: 9081,
                            https: 9443,
                        },
                        "80, 8081, 9081",
                        "443, 8443, 9443",
                        "80, 443, 8081, 8443, 9081, 9443",
                    ),
                ] {
                    let script = render(&lan, 5353, enabled, &policy, &[], ports);
                    let clear = script.find("meta mark set meta mark & 0xfffcffff").unwrap();
                    let direct = script
                        .find("ip daddr 198.18.0.0 meta mark set (meta mark & 0xfffcffff) | 65536")
                        .unwrap();
                    let fake = script.find("ip daddr @fake_direct").unwrap();
                    assert!(clear < direct && direct < fake);
                    assert!(script.contains(
                        "meta mark & 0x30000 == 65536 ct mark set (ct mark & 0xfffcffff) | 65536"
                    ));
                    let dns = script
                        .find("add chain inet gofro_routing gofro_dns")
                        .unwrap();
                    for condition in [
                        format!("iifname != \"{device}\""),
                        "meta l4proto != tcp".into(),
                        format!("tcp dport != {{ {allowed} }}"),
                    ] {
                        let drop = script
                            .find(&format!("gofro_mark ip daddr 198.18.0.0 {condition} drop"))
                            .unwrap();
                        assert!(drop < direct && drop < dns);
                    }
                    let dnat = script
                        .lines()
                        .filter(|line| line.contains("gofro_dnat") && line.contains("add rule"))
                        .collect::<Vec<_>>();
                    assert_eq!(
                        dnat,
                        vec![
                            format!(
                                "add rule inet gofro_routing gofro_dnat iifname \"{device}\" ip daddr 198.18.0.0 tcp dport {{ {http_sources} }} dnat ip to 100.64.0.1:{}",
                                ports.http
                            ),
                            format!(
                                "add rule inet gofro_routing gofro_dnat iifname \"{device}\" ip daddr 198.18.0.0 tcp dport {{ {https_sources} }} dnat ip to 100.64.0.1:{}",
                                ports.https
                            ),
                            format!(
                                "add rule inet gofro_routing gofro_dnat iifname \"{device}\" dnat ip to ip daddr map @fake_to_real"
                            ),
                        ]
                    );
                }
            }
        }
    }
}

#[test]
fn renders_kernel_only_split_routing() {
    let policy = RoutingPolicy::compile(
        RoutingConfig {
            domain_rules: vec![],
            ip_rules: vec![IpRule {
                name: "Block LAN".into(),
                enabled: true,
                matcher: IpMatch::Cidr {
                    value: "10.0.0.0/8".into(),
                },
                target: RouteTarget::Block,
            }],
            default_target: RouteTarget::Vpn,
            mode: RoutingMode::Rules,
            rule_order: None,
        },
        Arc::new(GeoData::default()),
    )
    .unwrap();
    let lan = LanContext {
        device: "br-home".into(),
        address: "192.168.0.1".parse().unwrap(),
        subnet: "192.168.0.0/24".parse().unwrap(),
    };
    let script = render(
        &lan,
        5353,
        true,
        &policy,
        &[FakeMapping {
            fake: "198.18.0.1".parse().unwrap(),
            real: "1.1.1.1".parse().unwrap(),
            target: RouteTarget::Direct,
        }],
        PANEL_PORTS,
    );
    assert!(script.contains("198.18.0.1 : 1.1.1.1"));
    assert!(script.contains("add element inet gofro_routing fake_direct { 198.18.0.1 }"));
    assert!(script.starts_with("destroy table inet gofro_routing\nadd table"));
    assert!(script.contains("iifname \"br-home\" ip daddr 192.168.0.0/24"));
    assert!(
        script.contains("meta mark & 0x30000 == 0 meta mark set (meta mark & 0xfffcffff) | 131072")
    );
    assert!(script.contains("meta mark & 0x30000 == 196608 drop"));
    assert!(script.contains("224.0.0.0/4"));
    assert!(script.contains("255.255.255.255/32"));
    let local = script.find("127.0.0.0/8").unwrap();
    let custom = script
        .find("ip daddr 10.0.0.0/8 meta mark set (meta mark & 0xfffcffff) | 196608")
        .unwrap();
    assert!(local < custom);
    assert!(script.contains("add chain inet gofro_routing gofro_mark"));
    assert!(script.contains("add chain inet gofro_routing gofro_dnat"));
    assert!(script.contains("add chain inet gofro_routing gofro_dns"));
    assert!(script.contains("udp dport 53 redirect to :5353"));
    assert!(script.contains("ct direction reply"));
    assert!(script.contains("meta mark & 0xfffcffff"));
    assert!(script.contains("gofro_ipv6"));
    assert!(!script.contains("gofro_routing mark"));
    assert!(script.contains("dnat ip to ip daddr map @fake_to_real"));
    let guard = render_guard(&lan);
    assert!(guard.contains("table inet gofro_guard"));
    assert!(guard.contains("iifname \"br-home\" oifname != \"br-home\" drop"));
}

#[test]
fn renders_ip_rules_in_effective_order_and_none_in_all_mode() {
    let config = RoutingConfig {
        domain_rules: vec![],
        ip_rules: vec![
            IpRule {
                name: "First".into(),
                enabled: true,
                matcher: IpMatch::Cidr {
                    value: "1.0.0.0/8".into(),
                },
                target: RouteTarget::Direct,
            },
            IpRule {
                name: "Second".into(),
                enabled: true,
                matcher: IpMatch::Cidr {
                    value: "1.1.0.0/16".into(),
                },
                target: RouteTarget::Block,
            },
        ],
        default_target: RouteTarget::Direct,
        mode: RoutingMode::Rules,
        rule_order: Some(vec![RuleRef::Ip { index: 1 }, RuleRef::Ip { index: 0 }]),
    };
    let policy = RoutingPolicy::compile(config.clone(), Arc::new(GeoData::default())).unwrap();
    let lan = LanContext {
        device: "br-home".into(),
        address: "192.168.0.1".parse().unwrap(),
        subnet: "192.168.0.0/24".parse().unwrap(),
    };
    let script = render(&lan, 5353, true, &policy, &[], PANEL_PORTS);
    assert!(script.find("1.1.0.0/16").unwrap() < script.find("1.0.0.0/8").unwrap());
    let mut all = config;
    all.mode = RoutingMode::All;
    let policy = RoutingPolicy::compile(all, Arc::new(GeoData::default())).unwrap();
    let script = render(&lan, 5353, false, &policy, &[], PANEL_PORTS);
    assert!(!script.contains("1.1.0.0/16"));
    assert!(script.contains("| 65536"));
    assert!(!script.contains("gofro_ipv6"));
}

#[test]
fn cached_vpn_fake_ips_become_direct_after_mode_flip() {
    let policy = RoutingPolicy::compile(
        RoutingConfig {
            domain_rules: vec![],
            ip_rules: vec![],
            default_target: RouteTarget::Vpn,
            mode: RoutingMode::Rules,
            rule_order: None,
        },
        Arc::new(GeoData::default()),
    )
    .unwrap();
    let lan = LanContext {
        device: "br-home".into(),
        address: "192.168.0.1".parse().unwrap(),
        subnet: "192.168.0.0/24".parse().unwrap(),
    };

    let script = render(
        &lan,
        5353,
        false,
        &policy,
        &[FakeMapping {
            fake: "198.18.0.1".parse().unwrap(),
            real: "1.1.1.1".parse().unwrap(),
            target: RouteTarget::Vpn,
        }],
        PANEL_PORTS,
    );

    assert!(script.contains("add element inet gofro_routing fake_vpn { 198.18.0.1 }"));
    assert!(script.contains("ip daddr @fake_vpn meta mark set (meta mark & 0xfffcffff) | 65536"));
    assert!(script.contains(&render_mappings(&[FakeMapping {
        fake: "198.18.0.1".parse().unwrap(),
        real: "1.1.1.1".parse().unwrap(),
        target: RouteTarget::Vpn,
    }])));
    let on = render(
        &lan,
        5353,
        true,
        &policy,
        &[FakeMapping {
            fake: "198.18.0.1".parse().unwrap(),
            real: "1.1.1.1".parse().unwrap(),
            target: RouteTarget::Vpn,
        }],
        PANEL_PORTS,
    );
    assert!(on.contains("add element inet gofro_routing fake_vpn { 198.18.0.1 }"));
    assert!(on.contains("ip daddr @fake_vpn meta mark set (meta mark & 0xfffcffff) | 131072"));
}

#[test]
fn incremental_install_and_expiry_use_the_same_intent_sets() {
    let mappings = [
        FakeMapping {
            fake: "198.18.0.1".parse().unwrap(),
            real: "1.1.1.1".parse().unwrap(),
            target: RouteTarget::Direct,
        },
        FakeMapping {
            fake: "198.18.0.2".parse().unwrap(),
            real: "8.8.8.8".parse().unwrap(),
            target: RouteTarget::Vpn,
        },
        FakeMapping {
            fake: "198.18.0.3".parse().unwrap(),
            real: "9.9.9.9".parse().unwrap(),
            target: RouteTarget::Block,
        },
    ];
    let added = render_mappings(&mappings);
    assert!(added.contains(
        "fake_to_real { 198.18.0.1 : 1.1.1.1, 198.18.0.2 : 8.8.8.8, 198.18.0.3 : 9.9.9.9 }"
    ));
    assert!(added.ends_with("add element inet gofro_routing fake_direct { 198.18.0.1 }\nadd element inet gofro_routing fake_vpn { 198.18.0.2 }\nadd element inet gofro_routing fake_block { 198.18.0.3 }\n"));
    assert_eq!(
        render_target_elements(&mappings, "delete"),
        "delete element inet gofro_routing fake_direct { 198.18.0.1 }\ndelete element inet gofro_routing fake_vpn { 198.18.0.2 }\ndelete element inet gofro_routing fake_block { 198.18.0.3 }\n"
    );
    assert!(render_mappings(&[]).is_empty());
}

#[test]
fn reclassifies_original_packets_without_clobbering_foreign_marks() {
    let policy = RoutingPolicy::compile(
        RoutingConfig {
            domain_rules: vec![],
            ip_rules: vec![],
            ..RoutingConfig::default()
        },
        Arc::new(GeoData::default()),
    )
    .unwrap();
    let lan = LanContext {
        device: "lan0".into(),
        address: "192.168.0.1".parse().unwrap(),
        subnet: "192.168.0.0/24".parse().unwrap(),
    };
    let script = render(&lan, 5353, true, &policy, &[], PANEL_PORTS);
    let reply = script.find("ct direction reply return").unwrap();
    let clear = script.find("meta mark set meta mark & 0xfffcffff").unwrap();
    let local = script.find("fib daddr type local").unwrap();
    let fake = script.find("ip daddr @fake_direct").unwrap();
    assert!(reply < clear && clear < local && local < fake);
    assert!(!script.contains("ct mark & 0x30000 != 0"));
    assert!(!script.contains("fake_to_mark"));
    assert_eq!(
        script
            .lines()
            .filter(|line| line.contains("redirect to"))
            .collect::<Vec<_>>(),
        vec![
            "add rule inet gofro_routing gofro_dns iifname \"lan0\" udp dport 53 redirect to :5353",
            "add rule inet gofro_routing gofro_dns iifname \"lan0\" tcp dport 53 redirect to :5353",
        ]
    );
    for old in [DIRECT_MARK, VPN_MARK, BLOCK_MARK] {
        for (target, enabled, expected) in [
            (RouteTarget::Vpn, true, VPN_MARK),
            (RouteTarget::Vpn, false, DIRECT_MARK),
            (RouteTarget::Block, false, BLOCK_MARK),
        ] {
            let mark = target_mark(effective_target(target, enabled));
            assert_eq!(((old | 4) & 0xfffcffff) | mark, expected | 4);
            assert_eq!(((old | 8) & 0xfffcffff) | mark, expected | 8);
        }
    }
}

#[test]
#[ignore = "emits nft fixtures only when explicitly requested by the Linux namespace smoke test"]
fn emit_netns_fixtures() {
    let directory = env::var("GOFRO_NFT_FIXTURE_DIR")
        .expect("GOFRO_NFT_FIXTURE_DIR is required for fixture emission");
    let policy = RoutingPolicy::compile(
        RoutingConfig {
            domain_rules: vec![],
            ip_rules: vec![
                IpRule {
                    name: "WAN direct".into(),
                    enabled: true,
                    matcher: IpMatch::Cidr {
                        value: "198.51.100.0/24".into(),
                    },
                    target: RouteTarget::Direct,
                },
                IpRule {
                    name: "Blocked WAN".into(),
                    enabled: true,
                    matcher: IpMatch::Cidr {
                        value: "203.0.113.0/24".into(),
                    },
                    target: RouteTarget::Block,
                },
            ],
            default_target: RouteTarget::Vpn,
            mode: RoutingMode::Rules,
            rule_order: None,
        },
        Arc::new(GeoData::default()),
    )
    .unwrap();
    let lan = LanContext {
        device: "lan0".into(),
        address: "192.168.0.1".parse().unwrap(),
        subnet: "192.168.0.0/24".parse().unwrap(),
    };
    let mappings = [FakeMapping {
        fake: "198.18.0.1".parse().unwrap(),
        real: "198.51.100.2".parse().unwrap(),
        target: RouteTarget::Direct,
    }];

    fs::write(
        format!("{directory}/routing.nft"),
        render(&lan, 5353, true, &policy, &mappings, PANEL_PORTS),
    )
    .unwrap();
    fs::write(format!("{directory}/guard.nft"), render_guard(&lan)).unwrap();
    let exclusions = ["02:00:00:00:00:02".parse().unwrap()];
    for (name, script) in [
        (
            "excluded",
            render_with_exclusions(
                &lan,
                5353,
                true,
                &policy,
                &mappings,
                PANEL_PORTS,
                &exclusions,
            ),
        ),
        (
            "guard-excluded",
            render_guard_with_exclusions(&lan, &exclusions),
        ),
        (
            "publish-excluded",
            render_publish_exclusions(&lan, &exclusions),
        ),
        ("publish-empty", render_publish_exclusions(&lan, &[])),
    ] {
        fs::write(format!("{directory}/{name}.nft"), script).unwrap();
    }
    fs::write(
        format!("{directory}/off.nft"),
        render(&lan, 5353, false, &policy, &mappings, PANEL_PORTS),
    )
    .unwrap();
    let mut all = policy.config().clone();
    all.mode = RoutingMode::All;
    let all = RoutingPolicy::compile(all, Arc::new(GeoData::default())).unwrap();
    fs::write(
        format!("{directory}/all.nft"),
        render(&lan, 5353, true, &all, &mappings, PANEL_PORTS),
    )
    .unwrap();
}
