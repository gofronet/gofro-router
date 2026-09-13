use std::{env, fs, sync::Arc};

use super::*;
use crate::{
    geodata::GeoData,
    model::{IpMatch, IpRule, RoutingConfig, RoutingMode, RuleRef},
};

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
    let script = render(&lan, 5353, true, &policy, &[]);
    assert!(script.find("1.1.0.0/16").unwrap() < script.find("1.0.0.0/8").unwrap());
    let mut all = config;
    all.mode = RoutingMode::All;
    let policy = RoutingPolicy::compile(all, Arc::new(GeoData::default())).unwrap();
    let script = render(&lan, 5353, false, &policy, &[]);
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
    let script = render(&lan, 5353, true, &policy, &[]);
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
        render(&lan, 5353, true, &policy, &mappings),
    )
    .unwrap();
    fs::write(format!("{directory}/guard.nft"), render_guard(&lan)).unwrap();
    fs::write(
        format!("{directory}/off.nft"),
        render(&lan, 5353, false, &policy, &mappings),
    )
    .unwrap();
    let mut all = policy.config().clone();
    all.mode = RoutingMode::All;
    let all = RoutingPolicy::compile(all, Arc::new(GeoData::default())).unwrap();
    fs::write(
        format!("{directory}/all.nft"),
        render(&lan, 5353, true, &all, &mappings),
    )
    .unwrap();
}
