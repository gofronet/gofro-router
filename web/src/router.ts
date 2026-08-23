import { createRouter, Router } from "sv-router";

import Layout from "./layout.svelte";
import { Home, Servers, Analytics, WifiSettings } from "./pages";

export const { p, navigate, isActive, route } = createRouter({
  layout: Layout,
  "/": Home,
  "/analytics": Analytics,
  "/servers": Servers,
  "/wifi": WifiSettings,
  "*": Home,
}, { base: "#" });

export { Router };
