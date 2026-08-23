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

export const statusSchema = z.object({
  version: z.string(),
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
});

export const updateStatusSchema = z.object({
  installed_version: z.string(),
  schema: z.literal(1),
  state: z.enum([
    "idle",
    "checking",
    "available",
    "downloading",
    "installing",
    "success",
    "error",
  ]),
  version: z.string().optional(),
  message: z.string().optional(),
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
export type UpdateStatus = z.infer<typeof updateStatusSchema>;
