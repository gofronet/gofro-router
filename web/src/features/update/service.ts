import { api } from "../../api";

export const updateService = {
  get: () => api.update.get(),
  check: () => api.update.check(),
  start: () => api.update.start(),
};
