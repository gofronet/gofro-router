import { createRouter, Router } from "sv-router";

import Layout from "./layout.svelte";
import { Home, Servers, Analytics, Routing, WifiSettings } from "./pages";

export const { p, navigate, isActive, route } = createRouter({
  layout: Layout,
  "/": Home,
  "/analytics": Analytics,
  "/servers": Servers,
  "/routing": Routing,
  "/wifi": WifiSettings,
  "*": Home,
}, { base: "#" });

export { Router };
