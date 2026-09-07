import { http, request } from "./client";
import {
  profileInputSchema,
  profileSchema,
  authStatusSchema,
  serverProbeSchema,
  serverInputSchema,
  serverVersionSchema,
  routingConfigSchema,
  routingTestSchema,
  statusSchema,
  wifiInputSchema,
  type ProfileInput,
  type Profile,
  type AuthStatus,
  type ServerProbe,
  type ServerInput,
  type ServerVersion,
  type RoutingConfig,
  type RoutingTest,
  type Status,
  type WifiBand,
  type WifiInput,
} from "./schemas";

const statusRequest = (factory: () => Promise<{ data: unknown }>) =>
  request(statusSchema, factory);
const mutation = { timeout: 0 };

export const api = {
  auth: {
    status: (): Promise<AuthStatus> => request(authStatusSchema, () => http.get("/auth/status")),
    setup: (setupCode: string, password: string): Promise<AuthStatus> =>
      request(authStatusSchema, () => http.post("/auth/setup", { setup_code: setupCode, password }, mutation)),
    login: (password: string): Promise<AuthStatus> =>
      request(authStatusSchema, () => http.post("/auth/login", { password }, mutation)),
    logout: (): Promise<AuthStatus> =>
      request(authStatusSchema, () => http.post("/auth/logout", {}, mutation)),
  },
  status: {
    get: (): Promise<Status> => statusRequest(() => http.get("/status")),
  },
  update: {
    start: (): Promise<Status> =>
      statusRequest(() => http.post("/update", {}, mutation)),
  },
  mode: {
    set: (vpnEnabled: boolean): Promise<Status> =>
      statusRequest(() =>
        http.post("/mode", { vpn_enabled: vpnEnabled }, mutation),
      ),
  },
  servers: {
    import: (input: ProfileInput): Promise<Status> => {
      const body = profileInputSchema.parse(input);
      return statusRequest(() => http.post("/servers/import", body, mutation));
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
    probe: (host: string, port: number): Promise<ServerProbe> =>
      request(serverProbeSchema, () =>
        http.post("/servers/probe", { host, port }, mutation),
      ),
    bootstrap: (
      name: string,
      host: string,
      port: number,
      password: string,
      hostKey: string,
    ): Promise<Status> =>
      statusRequest(() =>
        http.post(
          "/servers/bootstrap",
          { name, host, port, password, host_key: hostKey },
          mutation,
        ),
      ),
    check: (publicKey: string): Promise<ServerVersion> =>
      request(serverVersionSchema, () =>
        http.post("/servers/check", { public_key: publicKey }, mutation),
      ),
    updateManaged: (publicKey: string): Promise<ServerVersion> =>
      request(serverVersionSchema, () =>
        http.post("/servers/update-managed", { public_key: publicKey }, mutation),
      ),
    createProfile: (publicKey: string): Promise<Profile> =>
      request(profileSchema, () =>
        http.post("/servers/create-profile", { public_key: publicKey }, mutation),
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
  ProfileInput,
  Profile,
  AuthStatus,
  Server,
  ServerProbe,
  ServerInput,
  ServerVersion,
  Status,
  WifiBand,
  WifiInput,
} from "./schemas";
