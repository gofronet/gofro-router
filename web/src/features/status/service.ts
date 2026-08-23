import { api } from "../../api";

export const statusService = {
  get: () => api.status.get(),
};
