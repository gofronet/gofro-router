import { api, type ServerInput } from "../../api";

export const serverService = {
  create: (input: ServerInput) => api.servers.create(input),
  update: (previousPublicKey: string, input: ServerInput) =>
    api.servers.update(previousPublicKey, input),
  select: (publicKey: string) => api.servers.select(publicKey),
  remove: (publicKey: string) => api.servers.remove(publicKey),
};
