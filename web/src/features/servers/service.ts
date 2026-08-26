import { api, type ProfileInput, type ServerInput } from "../../api";

export const serverService = {
  import: (input: ProfileInput) => api.servers.import(input),
  update: (previousPublicKey: string, input: ServerInput) =>
    api.servers.update(previousPublicKey, input),
  select: (publicKey: string) => api.servers.select(publicKey),
  remove: (publicKey: string) => api.servers.remove(publicKey),
};
