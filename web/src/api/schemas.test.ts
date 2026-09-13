import assert from "node:assert/strict";
import {
  domainRuleInputSchema, ipRuleInputSchema, routingConfigInputSchema,
  routingConfigSchema, routingNameInputSchema, serverInputSchema, serverSchema, vpsHostInputSchema,
  macSchema, deviceExclusionsSchema, statusSchema,
} from "./schemas";

declare function test(name: string, run: () => void): void;

test("device exclusions normalize unicast MACs and stay outside destination packs", () => {
  assert.equal(macSchema.parse(" 02:AB:cd:EF:01:23 "), "02:ab:cd:ef:01:23");
  for (const mac of ["00:00:00:00:00:01", "fe:ff:ff:ff:ff:ff"]) assert.ok(macSchema.safeParse(mac).success);
  for (const mac of ["00:00:00:00:00:00", "ff:ff:ff:ff:ff:ff", "01:00:5e:00:00:01", "33:33:00:00:00:01", "02-ab-cd-ef-01-23", "2:ab:cd:ef:01:23", "02:gg:cd:ef:01:23", "02:ab:cd:ef:01:23:45"]) assert.equal(macSchema.safeParse(mac).success, false, mac);
  assert.ok(deviceExclusionsSchema.safeParse(Array(256).fill("02:00:00:00:00:01")).success);
  assert.equal(deviceExclusionsSchema.safeParse(Array(257).fill("02:00:00:00:00:01")).success, false);
  assert.deepEqual(statusSchema.shape.device_exclusions.parse(undefined), []);
  assert.equal("device_exclusions" in routingConfigSchema.parse({ domain_rules: [], ip_rules: [], default_target: "vpn", device_exclusions: ["02:00:00:00:00:01"] }), false);
});

test("routing input names count Unicode scalars and use Rust whitespace/control semantics", () => {
  for (const character of ["a", "\u044f", "\u{1f680}"]) {
    assert.equal(routingNameInputSchema.parse(` \u0085\u00a0${character.repeat(64)}\u2003\n`), character.repeat(64));
    assert.equal(routingNameInputSchema.safeParse(character.repeat(65)).success, false);
  }
  for (const input of ["", " \t\n\u0085\u00a0\u2003", "a\0b", "a\tb", "a\nb", "a\u007fb", "a\u0085b", "a\u009fb", "\ud800"]) {
    assert.equal(routingNameInputSchema.safeParse(input).success, false, JSON.stringify(input));
  }
  for (const [input, expected] of [[" \tName\n", "Name"], ["a\u00a0b", "a\u00a0b"], ["\ufeffName\ufeff", "\ufeffName\ufeff"]]) {
    assert.equal(routingNameInputSchema.parse(input), expected);
  }
});

test("rule input schemas reuse name validation while legacy read schemas stay permissive", () => {
  const domain = { name: " \u0085Name ", enabled: true, matcher: { type: "suffix", value: "example.com" }, target: "vpn" };
  const ip = { ...domain, matcher: { type: "cidr", value: "10.0.0.0/8" } };
  assert.equal(domainRuleInputSchema.parse(domain).name, "Name");
  assert.equal(ipRuleInputSchema.parse(ip).name, "Name");
  const input = { domain_rules: [domain], ip_rules: [ip], default_target: "vpn" };
  assert.equal(routingConfigInputSchema.parse(input).domain_rules[0].name, "Name");
  for (const name of ["a".repeat(65), "a\u0085b", " "]) {
    assert.equal(domainRuleInputSchema.safeParse({ ...domain, name }).success, false);
    assert.equal(ipRuleInputSchema.safeParse({ ...ip, name }).success, false);
    const legacy = { ...input, domain_rules: [{ ...domain, name }] };
    assert.equal(routingConfigInputSchema.safeParse(legacy).success, false);
    assert.equal(routingConfigSchema.parse(legacy).domain_rules[0].name, name);
  }
  assert.equal(serverSchema.parse({ name: "Legacy", endpoint: "[2606:4700:4700::1111]:8443", public_key: "key", managed: false }).endpoint, "[2606:4700:4700::1111]:8443");
});

test("VPS input admits only canonical public IPv4", () => {
  for (const host of ["1.1.1.1", "8.8.8.8"]) assert.equal(vpsHostInputSchema.parse(host), host);
  for (const host of [
    "2606:4700:4700::1111", "[2606:4700:4700::1111]", "::ffff:1.1.1.1",
    "2001:db8::1", "fe80::1%eth0", "vpn.example.com", "01.1.1.1",
    "0.0.0.0", "10.0.0.1", "127.0.0.1", "100.64.0.1", "169.254.1.1",
    "172.16.0.1", "192.168.1.1", "192.0.2.1", "192.88.99.1",
    "198.18.0.1", "198.51.100.1", "203.0.113.1", "224.0.0.1", "255.255.255.255",
  ]) {
    const result = vpsHostInputSchema.safeParse(host);
    assert.equal(result.success, false, host);
    if (!result.success) assert.match(result.error.issues[0].message, /IPv4/);
  }
});

test("server edits reject IPv6 literals without excluding IPv4 hostnames", () => {
  const server = { name: "VPS", public_key: "key" };
  for (const endpoint of ["1.1.1.1:8443", "vpn.example.com:8443"]) {
    assert.equal(serverInputSchema.parse({ ...server, endpoint }).endpoint, endpoint);
  }
  for (const endpoint of ["[2606:4700:4700::1111]:8443", "2606:4700:4700::1111:8443", "[::ffff:1.1.1.1]:8443", "[fe80::1%eth0]:8443"]) {
    assert.equal(serverInputSchema.safeParse({ ...server, endpoint }).success, false);
  }
});
