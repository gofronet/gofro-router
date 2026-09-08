import { cloneRules, packRules, reorderRules } from "./routing-rules";
import type { RoutingConfig } from "../domain/models";

declare function test(name: string, run: () => void): void;

const config: RoutingConfig = {
  mode: "all",
  default_target: "direct",
  rule_order: null,
  domain_rules: [{ name: "Disabled domain", enabled: false, matcher: { type: "suffix", value: "example.com" }, target: "block" }],
  ip_rules: [{ name: "IP", enabled: true, matcher: { type: "cidr", value: "192.0.2.0/24" }, target: "vpn" }],
};

test("clone, reorder, and pack retain all routing rules", () => {
  let number = 0;
  const draft = reorderRules(cloneRules(config, () => `${number++}`), 1, 0);
  const packed = packRules(config.mode, config.default_target, draft);
  if (
    draft.map((rule) => rule.key).join() !== "1,0"
    || packed.mode !== "all"
    || packed.domain_rules[0]?.enabled
    || packed.ip_rules[0]?.matcher.value !== "192.0.2.0/24"
    || packed.rule_order?.map((rule) => `${rule.kind}:${rule.index}`).join() !== "ip:0,domain:0"
  ) throw new Error("routing rules were not retained when reordered");
});
