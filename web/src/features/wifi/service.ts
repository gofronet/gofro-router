import { api, type WifiInput } from "../../api";

export const wifiService = {
  save: (input: WifiInput) => api.wifi.save(input),
};
