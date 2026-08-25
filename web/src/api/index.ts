import { http, request } from "./client";
import {
  serverInputSchema,
  routingConfigSchema,
  routingTestSchema,
  statusSchema,
  wifiInputSchema,
  type ServerInput,
  type RoutingConfig,
  type RoutingTest,
  type Status,
  type WifiInput,
} from "./schemas";

const statusRequest = (factory: () => Promise<{ data: unknown }>) =>
  request(statusSchema, factory);
const mutation = { timeout: 0 };

export const api = {
  status: {
    get: (): Promise<Status> => statusRequest(() => http.get("/status")),
  },
  mode: {
    set: (vpnEnabled: boolean): Promise<Status> =>
      statusRequest(() =>
        http.post("/mode", { vpn_enabled: vpnEnabled }, mutation),
      ),
  },
  servers: {
    create: (input: ServerInput): Promise<Status> => {
      const body = serverInputSchema.parse(input);
      return statusRequest(() => http.post("/servers", body, mutation));
    },
    update: (previousPublicKey: string, input: ServerInput): Promise<Status> => {
      const body = serverInputSchema.parse(input);
      return statusRequest(() =>
        http.put(
          "/servers",
          { previous_public_key: previousPublicKey, ...body },
          mutation,
        ),
      );
    },
    select: (publicKey: string): Promise<Status> =>
      statusRequest(() =>
        http.post("/servers/select", { public_key: publicKey }, mutation),
      ),
    remove: (publicKey: string): Promise<Status> =>
      statusRequest(() =>
        http.delete("/servers", {
          data: { public_key: publicKey },
          ...mutation,
        }),
      ),
  },
  wifi: {
    save: (input: WifiInput): Promise<Status> => {
      const body = wifiInputSchema.parse(input);
      return statusRequest(() => http.post("/ap", body, mutation));
    },
  },
  routing: {
    save: (input: RoutingConfig): Promise<Status> => {
      const body = routingConfigSchema.parse(input);
      return statusRequest(() => http.post("/routing", body, mutation));
    },
    test: (value: string): Promise<RoutingTest> =>
      request(routingTestSchema, () => http.post("/routing/test", { value })),
  },
};

export { ApiError } from "./client";
export type {
  Device,
  HistoryPoint,
  DomainRule,
  IpRule,
  RouteTarget,
  RoutingConfig,
  RoutingTest,
  Server,
  ServerInput,
  Status,
  WifiInput,
} from "./schemas";
