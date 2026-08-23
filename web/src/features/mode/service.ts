import { api } from "../../api";

export const modeService = {
  set: (vpnEnabled: boolean) => api.mode.set(vpnEnabled),
};
