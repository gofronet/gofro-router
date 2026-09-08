import { createRouter, Router } from "sv-router";

import Layout from "./layout.svelte";
import { Home, Servers, ServerManagement, Analytics, Routing, WifiSettings, Devices, System } from "./pages";

export const { p, navigate, isActive, route } = createRouter({
  layout: Layout,
  "/": Home,
  "/analytics": Analytics,
  "/servers": Servers,
  "/server-management": ServerManagement,
  "/routing": Routing,
  "/wifi": WifiSettings,
  "/devices": Devices,
  "/system": System,
  "*": Home,
}, { base: "#" });

export function serverManagementPath(key: string): string {
  return `/?server=${encodeURIComponent(key)}${p("/server-management").slice(1)}`;
}

export { Router };
