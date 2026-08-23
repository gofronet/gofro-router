import { http, request, updaterHttp } from "./client";
import {
  serverInputSchema,
  statusSchema,
  updateStatusSchema,
  wifiInputSchema,
  type ServerInput,
  type Status,
  type UpdateStatus,
  type WifiInput,
} from "./schemas";

const statusRequest = (factory: () => Promise<{ data: unknown }>) =>
  request(statusSchema, factory);
const updateRequest = (factory: () => Promise<{ data: unknown }>) =>
  request(updateStatusSchema, factory);
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
  update: {
    get: (): Promise<UpdateStatus> =>
      updateRequest(() => updaterHttp.get("/status")),
    check: (): Promise<UpdateStatus> =>
      updateRequest(() => updaterHttp.post("/check")),
    start: (): Promise<UpdateStatus> =>
      updateRequest(() => updaterHttp.post("/start")),
  },
};

export { ApiError } from "./client";
export type {
  Device,
  HistoryPoint,
  Server,
  ServerInput,
  Status,
  UpdateStatus,
  WifiInput,
} from "./schemas";
