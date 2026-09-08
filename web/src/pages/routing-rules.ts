import type { DomainRule, IpRule, RouteTarget, RoutingConfig } from "../domain/models";

export type DraftRule =
  | { key: string; kind: "domain"; name: string; value: string; matcher: DomainRule["matcher"]["type"]; target: RouteTarget; enabled: boolean }
  | { key: string; kind: "ip"; name: string; value: string; matcher: IpRule["matcher"]["type"]; target: RouteTarget; enabled: boolean };

export function cloneRules(config: RoutingConfig, createKey: () => string): DraftRule[] {
  const domains: DraftRule[] = config.domain_rules.map((rule) => ({ key: createKey(), kind: "domain", name: rule.name, value: rule.matcher.value, matcher: rule.matcher.type, target: rule.target, enabled: rule.enabled }));
  const ips: DraftRule[] = config.ip_rules.map((rule) => ({ key: createKey(), kind: "ip", name: rule.name, value: rule.matcher.value, matcher: rule.matcher.type, target: rule.target, enabled: rule.enabled }));
  const byReference = { domain: domains, ip: ips };
  const order = config.rule_order ?? [...domains.map((_, index) => ({ kind: "domain" as const, index })), ...ips.map((_, index) => ({ kind: "ip" as const, index }))];
  return order.map((item) => byReference[item.kind][item.index]).filter((rule): rule is DraftRule => rule !== undefined);
}

export function packRules(mode: RoutingConfig["mode"], defaultTarget: RouteTarget, draft: DraftRule[]): RoutingConfig {
  const domain_rules: DomainRule[] = [];
  const ip_rules: IpRule[] = [];
  const rule_order: NonNullable<RoutingConfig["rule_order"]> = [];
  for (const rule of draft) {
    if (rule.kind === "domain") {
      rule_order.push({ kind: "domain", index: domain_rules.length });
      domain_rules.push({ name: rule.name.trim(), enabled: rule.enabled, matcher: { type: rule.matcher, value: rule.value.trim() }, target: rule.target });
    } else {
      rule_order.push({ kind: "ip", index: ip_rules.length });
      ip_rules.push({ name: rule.name.trim(), enabled: rule.enabled, matcher: { type: rule.matcher, value: rule.value.trim() }, target: rule.target });
    }
  }
  return { mode, default_target: defaultTarget, domain_rules, ip_rules, rule_order };
}

export function reorderRules(rules: DraftRule[], from: number, to: number): DraftRule[] {
  if (from === to || from < 0 || to < 0 || from >= rules.length || to >= rules.length) return rules;
  const next = [...rules];
  next.splice(to, 0, next.splice(from, 1)[0]);
  return next;
}
