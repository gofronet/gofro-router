use std::{net::Ipv4Addr, sync::Arc};

use anyhow::{Context, Result};
use ipnet::Ipv4Net;

use crate::{
    config::normalize_domain,
    geodata::GeoData,
    model::{DomainMatch, IpMatch, RouteTarget, RoutingConfig, RoutingTestResult},
};

#[derive(Clone, Debug)]
pub(crate) struct RoutingPolicy {
    config: RoutingConfig,
    geodata: Arc<GeoData>,
    ip_rules: Vec<Option<Ipv4Net>>,
}

impl RoutingPolicy {
    pub(crate) fn compile(config: RoutingConfig, geodata: Arc<GeoData>) -> Result<Self> {
        for rule in &config.domain_rules {
            if rule.enabled
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
                    if rule.enabled && !geodata.has_ip(value) {
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
        self.config
            .domain_rules
            .iter()
            .filter(|rule| rule.enabled)
            .find(|rule| match &rule.matcher {
                DomainMatch::Exact { value } => domain == value,
                DomainMatch::Suffix { value } => {
                    domain == value
                        || domain
                            .strip_suffix(value)
                            .is_some_and(|prefix| prefix.ends_with('.'))
                }
                DomainMatch::GeoSite { value } => self.geodata.matches_site(value, domain),
            })
            .map(|rule| (rule.target, rule.name.as_str()))
    }

    pub(crate) fn ip_target(&self, ip: Ipv4Addr) -> (RouteTarget, Option<&str>) {
        if ip.is_private() || ip.is_loopback() || ip.is_link_local() {
            return (RouteTarget::Direct, Some("Локальная сеть"));
        }
        self.config
            .ip_rules
            .iter()
            .zip(&self.ip_rules)
            .filter(|(rule, _)| rule.enabled)
            .find(|(rule, cidr)| match &rule.matcher {
                IpMatch::Cidr { .. } => cidr.is_some_and(|network| network.contains(&ip)),
                IpMatch::GeoIp { value } => self.geodata.matches_ip(value, ip),
            })
            .map_or((self.config.default_target, None), |(rule, _)| {
                (rule.target, Some(rule.name.as_str()))
            })
    }

    pub(crate) fn target(&self, domain: &str, ip: Ipv4Addr) -> RouteTarget {
        self.domain_target(domain)
            .map_or_else(|| self.ip_target(ip).0, |(target, _)| target)
    }

    pub(crate) fn test(&self, value: &str) -> Result<RoutingTestResult> {
        let value = value.trim();
        if let Ok(ip) = value.parse::<Ipv4Addr>() {
            let (target, rule) = self.ip_target(ip);
            return Ok(RoutingTestResult {
                value: ip.to_string(),
                target,
                matched_rule: rule.map(str::to_owned),
            });
        }
        let domain = normalize_domain(value)?;
        let (target, rule) = self
            .domain_target(&domain)
            .map_or((self.config.default_target, None), |(target, rule)| {
                (target, Some(rule))
            });
        Ok(RoutingTestResult {
            value: domain,
            target,
            matched_rule: rule.map(str::to_owned),
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
}
