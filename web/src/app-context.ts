import { createContext } from "svelte";
import type { RouterState } from "./stores/router-state.svelte";

export type AppContext = RouterState;

export const [getAppContext, setAppContext] = createContext<AppContext>();
