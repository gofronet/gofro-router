import { bootstrapStream, http, request, setCsrfToken } from "./client";
import {
  profileInputSchema,
  deviceExclusionInputSchema,
  lanDevicesSchema,
  type DeviceExclusionInput,
  type LanDevices,
  profileSchema,
  authStatusSchema,
  serverInputSchema,
  serverVersionSchema,
  managedServerStatusSchema,
  friendNameSchema,
  routingConfigSchema,
  routingTestSchema,
  statusSchema,
  type ProfileInput,
  type Profile,
  type AuthStatus,
  type BootstrapStage,
  type ServerInput,
  type ServerVersion,
  type ManagedServerStatus,
  type RoutingConfig,
  type RoutingTest,
  type Status,
  type OnboardingStatus,
  onboardingStatusSchema,
} from "./schemas";

const statusRequest = (factory: () => Promise<{ data: unknown }>) =>
  request(statusSchema, factory);
const mutation = { timeout: 5 * 60_000 };
const authentication = { timeout: 30_000 };
// SSH commands allow 2 minutes; updates allow 16 minutes plus a version check.
const inspection = { timeout: 150_000 };
const managedUpdate = { timeout: 20 * 60_000 };

let pendingAuth: Promise<void> = Promise.resolve();
let authUnverified = false;
function authRequest(factory: () => Promise<{ data: unknown }>, readStatus = false): Promise<AuthStatus> {
  // Generation checks cannot undo Set-Cookie. Finish/abort each transport before
  // starting another, and use its validated CSRF token for the next request.
  const result = pendingAuth.then(async () => {
    if (authUnverified && !readStatus) {
      const status = await request(authStatusSchema, () => http.get("/auth/status"));
      setCsrfToken(status.csrf_token);
      authUnverified = false;
    }
    // Headers may change cookies even when the body or HTTP status is an error.
    authUnverified = true;
    const auth = await request(authStatusSchema, factory);
    setCsrfToken(auth.csrf_token);
    authUnverified = false;
    return auth;
  });
  pendingAuth = result.then(() => {}, () => {});
  return result;
}

export const api = {
  auth: {
    status: (): Promise<AuthStatus> => authRequest(() => http.get("/auth/status"), true),
    setup: (password: string, setupCode?: string): Promise<AuthStatus> =>
      authRequest(() => http.post("/auth/setup", { password, ...(setupCode ? { setup_code: setupCode } : {}) }, authentication)),
    login: (password: string): Promise<AuthStatus> =>
      authRequest(() => http.post("/auth/login", { password }, authentication)),
    changePassword: (currentPassword: string, password: string): Promise<AuthStatus> =>
      authRequest(() => http.post("/auth/password", { current_password: currentPassword, password }, authentication)),
    logout: (): Promise<AuthStatus> =>
      authRequest(() => http.post("/auth/logout", {}, authentication)),
  },
  status: {
    get: (): Promise<Status> => statusRequest(() => http.get("/status")),
  },
  deviceExclusions: {
    set: (input: DeviceExclusionInput): Promise<Status> => {
      const body = deviceExclusionInputSchema.parse(input);
      return statusRequest(() => http.post("/device-exclusions", body, mutation));
    },
  },
  lanDevices: {
    get: (): Promise<LanDevices> => request(lanDevicesSchema, () => http.get("/lan-devices")),
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
    bootstrap: (
      name: string,
      host: string,
      port: number,
      password: string,
      onStage: (stage: BootstrapStage) => void,
    ): Promise<Status> =>
      bootstrapStream({ name, host, port, password }, onStage),
    check: (publicKey: string): Promise<ServerVersion> =>
      request(serverVersionSchema, () =>
        http.post("/servers/check", { public_key: publicKey }, inspection),
      ),
    updateManaged: (publicKey: string): Promise<ServerVersion> =>
      request(serverVersionSchema, () =>
        http.post("/servers/update-managed", { public_key: publicKey }, managedUpdate),
      ),
    createProfile: (publicKey: string): Promise<Profile> =>
      request(profileSchema, () =>
        http.post("/servers/create-profile", { public_key: publicKey }, mutation),
      ),
    inspect: (publicKey: string): Promise<ManagedServerStatus> =>
      request(managedServerStatusSchema, () => http.post("/servers/management", { public_key: publicKey }, inspection)),
    restart: (publicKey: string): Promise<ManagedServerStatus> =>
      request(managedServerStatusSchema, () => http.post("/servers/restart", { public_key: publicKey }, mutation)),
    createFriend: (publicKey: string, name: string): Promise<ManagedServerStatus> =>
      request(managedServerStatusSchema, () => http.post("/servers/friends", { public_key: publicKey, name: friendNameSchema.parse(name) }, mutation)),
    renameFriend: (publicKey: string, peerKey: string, name: string): Promise<ManagedServerStatus> =>
      request(managedServerStatusSchema, () => http.put("/servers/friends", { public_key: publicKey, peer_key: peerKey, name: friendNameSchema.parse(name) }, mutation)),
    revokeFriend: (publicKey: string, peerKey: string): Promise<ManagedServerStatus> =>
      request(managedServerStatusSchema, () => http.delete("/servers/friends", { data: { public_key: publicKey, peer_key: peerKey }, ...mutation })),
    friendProfile: (publicKey: string, peerKey: string): Promise<Profile> =>
      request(profileSchema, () => http.post("/servers/friends/profile", { public_key: publicKey, peer_key: peerKey }, mutation)),
  },
  onboarding: {
    get: (): Promise<OnboardingStatus> =>
      request(onboardingStatusSchema, () => http.get("/onboarding")),
    complete: (): Promise<OnboardingStatus> =>
      request(onboardingStatusSchema, () => http.post("/onboarding/complete", {}, mutation)),
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
  DeviceExclusionInput,
  LanDevices,
  Mac,
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
  BootstrapStage,
  ServerInput,
  ServerVersion,
  FriendPeer,
  ManagedServerStatus,
  Status,
  OnboardingStatus,
} from "./schemas";
