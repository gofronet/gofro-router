import { z } from "zod";

export const serverSchema = z.object({
  name: z.string(),
  endpoint: z.string(),
  public_key: z.string(),
  managed: z.boolean(),
  emoji: z.string().optional(),
});

export const friendNameSchema = z.string().trim().min(1).refine(
  value => Array.from(value).length <= 60 && !/[\x00-\x1f\x7f-\x9f]/.test(value),
  "Имя должно содержать до 60 символов без управляющих знаков.",
);
export const friendPeerSchema = z.object({
  public_key: z.string().regex(/^[A-Za-z0-9+/]{43}=$/),
  name: friendNameSchema,
  revoked: z.boolean(),
  can_share: z.boolean(),
}).refine(peer => !peer.revoked || !peer.can_share);
export const managedServerStatusSchema = z.object({
  version: z.string(),
  peers: z.array(friendPeerSchema),
}).refine(status => new Set(status.peers.map(peer => peer.public_key)).size === status.peers.length);

export const serverProbeSchema = z.object({
  host: z.string(),
  port: z.number().int(),
  host_key: z.string(),
  fingerprint: z.string(),
});

export const vpsHostInputSchema = z.ipv4({ error: "Введите публичный IPv4-адрес VPS. IPv6 не поддерживается; получите IPv4 у провайдера VPS." }).refine(value => {
  const [a, b, c] = value.split(".").map(Number);
  return ![0, 10, 127].includes(a) && a < 224
    && !(a === 100 && b >= 64 && b <= 127)
    && !(a === 169 && b === 254)
    && !(a === 172 && b >= 16 && b <= 31)
    && !(a === 192 && (b === 0 || b === 168 || (b === 88 && c === 99)))
    && !(a === 198 && (b === 18 || b === 19 || (b === 51 && c === 100)))
    && !(a === 203 && b === 0 && c === 113);
}, "Введите публичный IPv4-адрес VPS из панели провайдера, без ведущих нулей.");

export const serverVersionSchema = z.object({
  version: z.string(),
  update_available: z.boolean(),
});

export const profileSchema = z.object({ profile: z.string() });

export const authStatusSchema = z.union([
  z.object({
    state: z.literal("setup"),
    csrf_token: z.string(),
    setup_method: z.literal("code"),
    setup_window_seconds: z.number().int().nonnegative(),
  }),
  z.object({
    state: z.literal("setup"),
    csrf_token: z.string(),
    setup_method: z.literal("local"),
    setup_window_seconds: z.number().int().nonnegative(),
  }),
  z.object({
    state: z.literal("setup"),
    csrf_token: z.string(),
    setup_method: z.literal("wifi_password"),
  }),
  z.object({ state: z.literal("login"), csrf_token: z.string() }),
  z.object({ state: z.literal("authenticated"), csrf_token: z.string() }),
]);

export const wifiBandSchema = z.enum(["2g", "5g"]);

// Legacy Wi-Fi states remain parseable for the paused development mock only.
export const onboardingStatusSchema = z.object({
  step: z.enum(["admin", "wifi", "wifi_applying", "server", "complete"]),
  networks: z.array(z.object({ band: wifiBandSchema, ssid: z.string() })),
  setup_window_seconds: z.number().int().nonnegative().nullable(),
  error: z.string().nullable(),
});

export const onboardingWifiInputSchema = z.object({
  networks: z.array(z.object({
    band: wifiBandSchema,
    ssid: z.string(),
    password: z.string(),
  })),
});

export const historyPointSchema = z.object({
  timestamp: z.number(),
  rx_bps: z.number(),
  tx_bps: z.number(),
  load_percent: z.number().optional(),
  memory_percent: z.number().optional(),
  temperature_c: z.number().nullable().optional(),
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
// Rust str::trim uses Unicode White_Space, unlike JavaScript trim (NEL/BOM differ).
export const routingNameInputSchema = z.string()
  .transform(value => value.replace(/^\p{White_Space}+|\p{White_Space}+$/gu, ""))
  .refine(value => Array.from(value).length >= 1 && Array.from(value).length <= 64
    && !/[\p{Cc}\p{Cs}]/u.test(value),
  "Название правила должно содержать от 1 до 64 символов без управляющих знаков.");
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
  mode: z.enum(["rules", "all"]).default("rules"),
  rule_order: z.array(z.object({ kind: z.enum(["domain", "ip"]), index: z.number().int().nonnegative() })).nullable().default(null),
  domain_rules: z.array(domainRuleSchema),
  ip_rules: z.array(ipRuleSchema),
  default_target: routeTargetSchema,
});
export const domainRuleInputSchema = domainRuleSchema.extend({ name: routingNameInputSchema });
export const ipRuleInputSchema = ipRuleSchema.extend({ name: routingNameInputSchema });
export const routingConfigInputSchema = routingConfigSchema.extend({
  domain_rules: z.array(domainRuleInputSchema),
  ip_rules: z.array(ipRuleInputSchema),
});
export const routingTestSchema = z.object({
  value: z.string(),
  target: routeTargetSchema,
  matched_rule: z.string().nullable(),
  scope: z.enum(["ip", "domain_preview"]).optional(),
});

export const rebootSchema = z.object({ rebooting: z.literal(true) });

const apStatusSchema = z.object({
  ssid: z.string().optional(),
  networks: z.array(z.object({
    band: wifiBandSchema,
    ssid: z.string(),
  })).optional(),
  address: z.string(),
  domain: z.string(),
}).refine((ap) => ap.networks !== undefined || ap.ssid !== undefined)
  .transform((ap) => ({
    networks: ap.networks ?? [{ band: undefined, ssid: ap.ssid! }],
    address: ap.address,
    domain: ap.domain,
  }));

export const statusSchema = z.object({
  version: z.string(),
  router_info: z.object({
    model: z.string().nullable(),
    os_name: z.string().nullable(),
    os_version: z.string().nullable(),
  }).optional(),
  update: z.object({
    running: z.boolean(),
    result: z.enum(["current", "updated", "failed"]).nullable(),
  }),
  vpn_enabled: z.boolean(),
  tunnel_active: z.boolean(),
  interface: z.string(),
  active_server_key: z.string().nullable(),
  servers: z.array(serverSchema),
  ap: apStatusSchema.optional().default({ networks: [], address: "", domain: "" }),
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
    load_percent: z.number().optional(),
    memory_percent: z.number().optional(),
    temperature_c: z.number().nullable().optional(),
    uptime_seconds: z.number().optional(),
    wifi_clients: z.number().optional(),
  }),
  history: z.array(historyPointSchema),
  devices: z.array(deviceSchema).default([]),
  routing: z.object({
    config: routingConfigSchema,
    dns_active: z.boolean(),
    fake_ips: z.number(),
    geosite_loaded: z.boolean(),
    geoip_loaded: z.boolean(),
    dataplane_active: z.boolean(),
    degraded: z.boolean().default(false),
  }),
});

export const serverInputSchema = serverSchema.pick({
  name: true,
  endpoint: true,
  public_key: true,
  emoji: true,
}).extend({
  endpoint: z.string().refine(value => !/[:\[\]]/.test(value.slice(0, value.lastIndexOf(":"))),
    "IPv6 endpoint не поддерживается. Укажите IPv4-адрес или имя сервера с IPv4 в формате host:port."),
});
export const bootstrapStageSchema = z.enum([
  "waiting", "host_key", "connect", "inspect", "install", "authorize", "profile", "save",
]);
export const bootstrapEventSchema = z.discriminatedUnion("type", [
  z.object({ type: z.literal("stage"), stage: bootstrapStageSchema }).strict(),
  z.object({ type: z.literal("complete"), status: statusSchema }).strict(),
  z.object({ type: z.literal("error"), stage: bootstrapStageSchema, message: z.string().trim().min(1).max(4096) }).strict(),
]);
export type BootstrapStage = z.infer<typeof bootstrapStageSchema>;
export type BootstrapEvent = z.infer<typeof bootstrapEventSchema>;
export const profileInputSchema = z.object({
  name: z.string(),
  profile: z.string(),
});
export const wifiInputSchema = z.object({
  band: wifiBandSchema.optional(),
  ssid: z.string(),
  password: z.string(),
});

export type Server = z.infer<typeof serverSchema>;
export type FriendPeer = z.infer<typeof friendPeerSchema>;
export type ManagedServerStatus = z.infer<typeof managedServerStatusSchema>;
export type ServerProbe = z.infer<typeof serverProbeSchema>;
export type ServerVersion = z.infer<typeof serverVersionSchema>;
export type Profile = z.infer<typeof profileSchema>;
export type AuthStatus = z.infer<typeof authStatusSchema>;
export type OnboardingStatus = z.infer<typeof onboardingStatusSchema>;
export type OnboardingWifiInput = z.infer<typeof onboardingWifiInputSchema>;
export type HistoryPoint = z.infer<typeof historyPointSchema>;
export type Device = z.infer<typeof deviceSchema>;
export type Status = z.infer<typeof statusSchema>;
export type ServerInput = z.infer<typeof serverInputSchema>;
export type ProfileInput = z.infer<typeof profileInputSchema>;
export type WifiInput = z.infer<typeof wifiInputSchema>;
export type WifiBand = z.infer<typeof wifiBandSchema>;
export type RouteTarget = z.infer<typeof routeTargetSchema>;
export type DomainRule = z.infer<typeof domainRuleSchema>;
export type IpRule = z.infer<typeof ipRuleSchema>;
export type RoutingConfig = z.infer<typeof routingConfigSchema>;
export type RoutingTest = z.infer<typeof routingTestSchema>;
