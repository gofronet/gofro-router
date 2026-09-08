import { createRouter, Router } from "sv-router";

import Layout from "./layout.svelte";
import { Home, Servers, Analytics, Routing, WifiSettings, Devices, System } from "./pages";

export const { p, navigate, isActive, route } = createRouter({
  layout: Layout,
  "/": Home,
  "/analytics": Analytics,
  "/servers": Servers,
  "/routing": Routing,
  "/wifi": WifiSettings,
  "/devices": Devices,
  "/system": System,
  "*": Home,
}, { base: "#" });

export { Router };
