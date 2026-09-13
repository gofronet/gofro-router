use std::{
    net::Ipv4Addr,
    sync::{Arc, LazyLock},
};

use anyhow::{Context, Result};
use ipnet::Ipv4Net;

use crate::{
    config::normalize_domain,
    geodata::GeoData,
    model::{
        DomainMatch, IpMatch, RouteTarget, RoutingConfig, RoutingMode, RoutingTestResult,
        RoutingTestScope, RuleRef,
    },
};

pub(crate) static LAN_RANGES: LazyLock<[Ipv4Net; 7]> = LazyLock::new(|| {
    [
        "10.0.0.0/8".parse().unwrap(),
        "127.0.0.0/8".parse().unwrap(),
        "169.254.0.0/16".parse().unwrap(),
        "172.16.0.0/12".parse().unwrap(),
        "192.168.0.0/16".parse().unwrap(),
        "224.0.0.0/4".parse().unwrap(),
        "255.255.255.255/32".parse().unwrap(),
    ]
});

pub(crate) fn is_lan_destination(ip: Ipv4Addr) -> bool {
    LAN_RANGES.iter().any(|range| range.contains(&ip))
}

#[derive(Clone, Debug)]
pub(crate) struct RoutingPolicy {
    config: RoutingConfig,
    geodata: Arc<GeoData>,
    ip_rules: Vec<Option<Ipv4Net>>,
}

impl RoutingPolicy {
    pub(crate) fn compile(config: RoutingConfig, geodata: Arc<GeoData>) -> Result<Self> {
        for rule in &config.domain_rules {
            if config.mode == RoutingMode::Rules
                && rule.enabled
                && let DomainMatch::GeoSite { value } = &rule.matcher
                && !geodata.has_site(value)
            {
                anyhow::bail!("GeoSite-тег {value} отсутствует в geosite.dat");
            }
        }
        let ip_rules = config
            .ip_rules
            .iter()
            .map(|rule| match &rule.matcher {
                IpMatch::Cidr { value } => value
                    .parse::<Ipv4Net>()
                    .map(Some)
                    .context("invalid normalized CIDR"),
                IpMatch::GeoIp { value } => {
                    if config.mode == RoutingMode::Rules && rule.enabled && !geodata.has_ip(value) {
                        anyhow::bail!("GeoIP-тег {value} отсутствует в geoip.dat");
                    }
                    Ok(None)
                }
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            config,
            geodata,
            ip_rules,
        })
    }

    pub(crate) fn config(&self) -> &RoutingConfig {
        &self.config
    }

    pub(crate) fn geodata(&self) -> &GeoData {
        &self.geodata
    }

    pub(crate) fn domain_target(&self, domain: &str) -> Option<(RouteTarget, &str)> {
        (self.config.mode == RoutingMode::Rules)
            .then(|| self.configured_domain_target(domain))
            .flatten()
    }

    pub(crate) fn configured_domain_target(&self, domain: &str) -> Option<(RouteTarget, &str)> {
        self.rule_refs()
            .filter_map(|reference| match reference {
                RuleRef::Domain { index } => self.config.domain_rules.get(index),
                RuleRef::Ip { .. } => None,
            })
            .find(|rule| rule.enabled && self.matches_domain(rule, domain))
            .map(|rule| (rule.target, rule.name.as_str()))
    }

    pub(crate) fn ip_target(&self, ip: Ipv4Addr) -> (RouteTarget, Option<&str>) {
        if is_lan_destination(ip) {
            return (RouteTarget::Direct, Some("Локальная сеть"));
        }
        if self.config.mode == RoutingMode::All {
            return (RouteTarget::Vpn, None);
        }
        self.ip_rule_indices()
            .find_map(|index| {
                let rule = &self.config.ip_rules[index];
                (rule.enabled && self.matches_ip(index, ip))
                    .then_some((rule.target, Some(rule.name.as_str())))
            })
            .unwrap_or((self.effective_fallback(), None))
    }

    pub(crate) fn target(&self, domain: &str, ip: Ipv4Addr) -> RouteTarget {
        if is_lan_destination(ip) {
            return RouteTarget::Direct;
        }
        if self.config.mode == RoutingMode::All {
            return RouteTarget::Vpn;
        }
        self.rule_refs()
            .find_map(|reference| match reference {
                RuleRef::Domain { index } => self
                    .config
                    .domain_rules
                    .get(index)
                    .filter(|rule| rule.enabled && self.matches_domain(rule, domain))
                    .map(|rule| rule.target),
                RuleRef::Ip { index } => self
                    .config
                    .ip_rules
                    .get(index)
                    .filter(|rule| rule.enabled && self.matches_ip(index, ip))
                    .map(|rule| rule.target),
            })
            .unwrap_or(self.effective_fallback())
    }

    pub(crate) fn effective_fallback(&self) -> RouteTarget {
        if self.config.mode == RoutingMode::All {
            RouteTarget::Vpn
        } else {
            self.config.default_target
        }
    }

    pub(crate) fn ip_rule_indices(&self) -> Box<dyn Iterator<Item = usize> + '_> {
        if self.config.mode == RoutingMode::All {
            return Box::new(std::iter::empty());
        }
        Box::new(self.rule_refs().filter_map(|reference| match reference {
            RuleRef::Ip { index } => Some(index),
            RuleRef::Domain { .. } => None,
        }))
    }

    fn rule_refs(&self) -> Box<dyn Iterator<Item = RuleRef> + '_> {
        match &self.config.rule_order {
            Some(order) => Box::new(order.iter().copied()),
            None => Box::new(
                (0..self.config.domain_rules.len())
                    .map(|index| RuleRef::Domain { index })
                    .chain((0..self.config.ip_rules.len()).map(|index| RuleRef::Ip { index })),
            ),
        }
    }

    fn matches_domain(&self, rule: &crate::model::DomainRule, domain: &str) -> bool {
        match &rule.matcher {
            DomainMatch::Exact { value } => domain == value,
            DomainMatch::Suffix { value } => {
                domain == value
                    || domain
                        .strip_suffix(value)
                        .is_some_and(|prefix| prefix.ends_with('.'))
            }
            DomainMatch::GeoSite { value } => self.geodata.matches_site(value, domain),
        }
    }

    fn matches_ip(&self, index: usize, ip: Ipv4Addr) -> bool {
        match &self.config.ip_rules[index].matcher {
            IpMatch::Cidr { .. } => {
                self.ip_rules[index].is_some_and(|network| network.contains(&ip))
            }
            IpMatch::GeoIp { value } => self.geodata.matches_ip(value, ip),
        }
    }

    pub(crate) fn test(&self, value: &str) -> Result<RoutingTestResult> {
        let value = value.trim();
        if let Ok(ip) = value.parse::<Ipv4Addr>() {
            let (target, rule) = self.ip_target(ip);
            return Ok(RoutingTestResult {
                value: ip.to_string(),
                target,
                matched_rule: rule.map(str::to_owned),
                scope: RoutingTestScope::Ip,
            });
        }
        let domain = normalize_domain(value)?;
        let (target, rule) = self
            // A hostname cannot know its eventual A record, so this deliberately previews only active domain rules.
            .domain_target(&domain)
            .map_or((self.effective_fallback(), None), |(target, rule)| {
                (target, Some(rule))
            });
        Ok(RoutingTestResult {
            value: domain,
            target,
            matched_rule: rule.map(str::to_owned),
            scope: RoutingTestScope::DomainPreview,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{DomainRule, IpRule};

    #[test]
    fn explicit_rules_override_russia_preset() {
        let config = RoutingConfig {
            domain_rules: vec![
                DomainRule {
                    name: "Force VPN".into(),
                    enabled: true,
                    matcher: DomainMatch::Exact {
                        value: "vk.com".into(),
                    },
                    target: RouteTarget::Vpn,
                },
                DomainRule {
                    name: "RU".into(),
                    enabled: true,
                    matcher: DomainMatch::Suffix { value: "ru".into() },
                    target: RouteTarget::Direct,
                },
            ],
            ip_rules: vec![IpRule {
                name: "LAN".into(),
                enabled: true,
                matcher: IpMatch::Cidr {
                    value: "10.0.0.0/8".into(),
                },
                target: RouteTarget::Direct,
            }],
            default_target: RouteTarget::Vpn,
            mode: RoutingMode::Rules,
            rule_order: None,
        };
        let policy = RoutingPolicy::compile(config, Arc::new(GeoData::default())).unwrap();
        assert_eq!(
            policy.target("vk.com", "1.1.1.1".parse().unwrap()),
            RouteTarget::Vpn
        );
        assert_eq!(
            policy.target("example.ru", "1.1.1.1".parse().unwrap()),
            RouteTarget::Direct
        );
        assert_eq!(
            policy.target("example.com", "10.0.0.1".parse().unwrap()),
            RouteTarget::Direct
        );
    }

    #[test]
    fn order_changes_domain_and_ip_decision_without_removing_disabled_rules() {
        let mut config = RoutingConfig {
            domain_rules: vec![DomainRule {
                name: "Domain".into(),
                enabled: true,
                matcher: DomainMatch::Exact {
                    value: "example.com".into(),
                },
                target: RouteTarget::Vpn,
            }],
            ip_rules: vec![IpRule {
                name: "IP".into(),
                enabled: true,
                matcher: IpMatch::Cidr {
                    value: "1.1.1.0/24".into(),
                },
                target: RouteTarget::Direct,
            }],
            default_target: RouteTarget::Block,
            mode: RoutingMode::Rules,
            rule_order: Some(vec![RuleRef::Ip { index: 0 }, RuleRef::Domain { index: 0 }]),
        };
        let policy = RoutingPolicy::compile(config.clone(), Arc::new(GeoData::default())).unwrap();
        assert_eq!(
            policy.target("example.com", "1.1.1.1".parse().unwrap()),
            RouteTarget::Direct
        );
        config.rule_order = Some(vec![RuleRef::Domain { index: 0 }, RuleRef::Ip { index: 0 }]);
        let policy = RoutingPolicy::compile(config.clone(), Arc::new(GeoData::default())).unwrap();
        assert_eq!(
            policy.target("example.com", "1.1.1.1".parse().unwrap()),
            RouteTarget::Vpn
        );
        config.domain_rules[0].enabled = false;
        let policy = RoutingPolicy::compile(config, Arc::new(GeoData::default())).unwrap();
        assert_eq!(
            policy.target("example.com", "1.1.1.1".parse().unwrap()),
            RouteTarget::Direct
        );
    }

    #[test]
    fn all_mode_keeps_config_but_sends_public_ips_to_vpn_and_lan_direct() {
        let config = RoutingConfig {
            mode: RoutingMode::All,
            ..RoutingConfig::default()
        };
        let policy = RoutingPolicy::compile(config.clone(), Arc::new(GeoData::default())).unwrap();
        assert_eq!(
            policy.target("anything.example", "1.1.1.1".parse().unwrap()),
            RouteTarget::Vpn
        );
        assert_eq!(
            policy.target("anything.example", "10.0.0.1".parse().unwrap()),
            RouteTarget::Direct
        );
        assert_eq!(
            policy.target("anything.example", "224.0.0.251".parse().unwrap()),
            RouteTarget::Direct
        );
        assert_eq!(
            policy.target("anything.example", "255.255.255.255".parse().unwrap()),
            RouteTarget::Direct
        );
        assert_eq!(
            policy.config().domain_rules.len(),
            config.domain_rules.len()
        );
        let domain = policy.test("anything.example").unwrap();
        assert_eq!(domain.target, RouteTarget::Vpn);
        assert_eq!(domain.matched_rule, None);
        let lan = policy.test("224.0.0.251").unwrap();
        assert_eq!(lan.target, RouteTarget::Direct);
        assert_eq!(lan.matched_rule.as_deref(), Some("Локальная сеть"));
    }

    #[test]
    fn route_test_labels_hostname_results_as_domain_previews() {
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
        assert!(matches!(
            policy.test("1.1.1.1").unwrap().scope,
            RoutingTestScope::Ip
        ));
        assert!(matches!(
            policy.test("example.com").unwrap().scope,
            RoutingTestScope::DomainPreview
        ));
    }
}
