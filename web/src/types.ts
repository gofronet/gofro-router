export type Server = {
  name: string;
  endpoint: string;
  public_key: string;
};

export type HistoryPoint = {
  timestamp: number;
  rx_bps: number;
  tx_bps: number;
  load_percent: number;
  memory_percent: number;
  temperature_c: number | null;
};

export type Device = {
  mac: string;
  ip: string | null;
  hostname: string | null;
  signal_dbm: number | null;
  rx_bytes: number;
  tx_bytes: number;
  rx_bps: number;
  tx_bps: number;
  rx_bitrate_mbps: number | null;
  tx_bitrate_mbps: number | null;
  connected_seconds: number;
  inactive_ms: number;
};

export type Status = {
  version: string;
  vpn_enabled: boolean;
  tunnel_active: boolean;
  interface: string;
  active_server_key: string | null;
  servers: Server[];
  ap: { ssid: string; address: string; domain: string };
  peer: null | {
    public_key: string;
    endpoint: string | null;
    allowed_ips: string[];
    latest_handshake: number | null;
    handshake_age_seconds: number | null;
    rx_bytes: number;
    tx_bytes: number;
    persistent_keepalive: number | null;
  };
  stats: {
    rx_bps: number;
    tx_bps: number;
    load_percent: number;
    memory_percent: number;
    temperature_c: number | null;
    uptime_seconds: number;
    wifi_clients: number;
  };
  history: HistoryPoint[];
  devices: Device[];
};

export type Mutate = (kind: string, path: string, init: RequestInit) => Promise<boolean>;
