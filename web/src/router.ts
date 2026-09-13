import { createRouter, Router } from "sv-router";

import Layout from "./layout.svelte";
import { Home, Servers, ServerManagement, Routing, System } from "./pages";

export const { p, navigate, isActive, route } = createRouter({
  layout: Layout,
  "/": Home,
  "/servers": Servers,
  "/server-management": ServerManagement,
  "/routing": Routing,
  "/system": System,
  "*": Home,
}, { base: "#" });

export function serverManagementPath(key: string): string {
  return `/?server=${encodeURIComponent(key)}${p("/server-management").slice(1)}`;
}

export { Router };
