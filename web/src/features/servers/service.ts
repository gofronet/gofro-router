import { api, type ProfileInput, type ServerInput } from "../../api";

export const serverService = {
  import: (input: ProfileInput) => api.servers.import(input),
  update: (previousPublicKey: string, input: ServerInput) =>
    api.servers.update(previousPublicKey, input),
  select: (publicKey: string) => api.servers.select(publicKey),
  remove: (publicKey: string) => api.servers.remove(publicKey),
  probe: (host: string, port: number) => api.servers.probe(host, port),
  bootstrap: (
    name: string,
    host: string,
    port: number,
    password: string,
    hostKey: string,
  ) => api.servers.bootstrap(name, host, port, password, hostKey),
  check: (publicKey: string) => api.servers.check(publicKey),
  updateManaged: (publicKey: string) => api.servers.updateManaged(publicKey),
  createProfile: (publicKey: string) => api.servers.createProfile(publicKey),
};
