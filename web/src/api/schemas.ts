import { z } from "zod";

export const serverSchema = z.object({
  name: z.string(),
  endpoint: z.string(),
  public_key: z.string(),
});

export const historyPointSchema = z.object({
  timestamp: z.number(),
  rx_bps: z.number(),
  tx_bps: z.number(),
  load_percent: z.number(),
  memory_percent: z.number(),
  temperature_c: z.number().nullable(),
});

export const deviceSchema = z.object({
  mac: z.string(),
  ip: z.string().nullable(),
  hostname: z.string().nullable(),
  signal_dbm: z.number().nullable(),
  rx_bytes: z.number(),
  tx_bytes: z.number(),
  rx_bps: z.number(),
  tx_bps: z.number(),
  rx_bitrate_mbps: z.number().nullable(),
  tx_bitrate_mbps: z.number().nullable(),
  connected_seconds: z.number(),
  inactive_ms: z.number(),
});

export const routeTargetSchema = z.enum(["direct", "vpn", "block"]);
export const domainRuleSchema = z.object({
  name: z.string(),
  enabled: z.boolean(),
  matcher: z.discriminatedUnion("type", [
    z.object({ type: z.literal("exact"), value: z.string() }),
    z.object({ type: z.literal("suffix"), value: z.string() }),
    z.object({ type: z.literal("geo_site"), value: z.string() }),
  ]),
  target: routeTargetSchema,
});
export const ipRuleSchema = z.object({
  name: z.string(),
  enabled: z.boolean(),
  matcher: z.discriminatedUnion("type", [
    z.object({ type: z.literal("cidr"), value: z.string() }),
    z.object({ type: z.literal("geo_ip"), value: z.string() }),
  ]),
  target: routeTargetSchema,
});
export const routingConfigSchema = z.object({
  domain_rules: z.array(domainRuleSchema),
  ip_rules: z.array(ipRuleSchema),
  default_target: routeTargetSchema,
});
export const routingTestSchema = z.object({
  value: z.string(),
  target: routeTargetSchema,
  matched_rule: z.string().nullable(),
});

export const statusSchema = z.object({
  version: z.string(),
  update: z.object({
    running: z.boolean(),
    result: z.enum(["current", "updated", "failed"]).nullable(),
  }),
  vpn_enabled: z.boolean(),
  tunnel_active: z.boolean(),
  interface: z.string(),
  active_server_key: z.string().nullable(),
  servers: z.array(serverSchema),
  ap: z.object({
    ssid: z.string(),
    address: z.string(),
    domain: z.string(),
  }),
  peer: z.object({
    public_key: z.string(),
    endpoint: z.string().nullable(),
    allowed_ips: z.array(z.string()),
    latest_handshake: z.number().nullable(),
    handshake_age_seconds: z.number().nullable(),
    rx_bytes: z.number(),
    tx_bytes: z.number(),
    persistent_keepalive: z.number().nullable(),
  }).nullable(),
  stats: z.object({
    rx_bps: z.number(),
    tx_bps: z.number(),
    load_percent: z.number(),
    memory_percent: z.number(),
    temperature_c: z.number().nullable(),
    uptime_seconds: z.number(),
    wifi_clients: z.number(),
  }),
  history: z.array(historyPointSchema),
  devices: z.array(deviceSchema),
  routing: z.object({
    config: routingConfigSchema,
    dns_active: z.boolean(),
    fake_ips: z.number(),
    geosite_loaded: z.boolean(),
    geoip_loaded: z.boolean(),
    dataplane_active: z.boolean(),
  }),
});

export const serverInputSchema = serverSchema;
export const wifiInputSchema = z.object({
  ssid: z.string(),
  password: z.string(),
});

export type Server = z.infer<typeof serverSchema>;
export type HistoryPoint = z.infer<typeof historyPointSchema>;
export type Device = z.infer<typeof deviceSchema>;
export type Status = z.infer<typeof statusSchema>;
export type ServerInput = z.infer<typeof serverInputSchema>;
export type WifiInput = z.infer<typeof wifiInputSchema>;
export type RouteTarget = z.infer<typeof routeTargetSchema>;
export type DomainRule = z.infer<typeof domainRuleSchema>;
export type IpRule = z.infer<typeof ipRuleSchema>;
export type RoutingConfig = z.infer<typeof routingConfigSchema>;
export type RoutingTest = z.infer<typeof routingTestSchema>;
