<script lang="ts">
  import type { Snippet } from "svelte";
  import ArrowDown from "lucide-svelte/icons/arrow-down";
  import ArrowUp from "lucide-svelte/icons/arrow-up";
  import Trash2 from "lucide-svelte/icons/trash-2";

  import type { RouteTarget } from "../domain/models";

  let {
    name = $bindable(),
    enabled = $bindable(),
    target = $bindable(),
    index,
    last,
    onup,
    ondown,
    onremove,
    children,
  }: {
    name: string;
    enabled: boolean;
    target: RouteTarget;
    index: number;
    last: number;
    onup: () => void;
    ondown: () => void;
    onremove: () => void;
    children: Snippet;
  } = $props();

  const targets: { value: RouteTarget; label: string }[] = [
    { value: "direct", label: "Напрямую" },
    { value: "vpn", label: "VPN" },
    { value: "block", label: "Блокировать" },
  ];
</script>

<article class={`grid gap-4 rounded-[24px] border bg-white p-4 shadow-sm sm:p-5 ${enabled ? "border-[#dedee1]" : "border-[#ececef] opacity-60"}`}>
  <div class="grid grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-3">
    <input class="size-5 accent-[#09090b]" type="checkbox" bind:checked={enabled} aria-label="Включить правило" />
    <input class="h-11 min-w-0 border-0 bg-transparent text-sm font-bold outline-none" bind:value={name} maxlength="64" required />
    <div class="flex">
      <button class="grid size-10 place-items-center border-0 bg-transparent" type="button" disabled={index === 0} onclick={onup} aria-label="Выше"><ArrowUp size={17} /></button>
      <button class="grid size-10 place-items-center border-0 bg-transparent" type="button" disabled={index === last} onclick={ondown} aria-label="Ниже"><ArrowDown size={17} /></button>
      <button class="grid size-10 place-items-center border-0 bg-transparent text-red-700" type="button" onclick={onremove} aria-label="Удалить"><Trash2 size={17} /></button>
    </div>
  </div>
  <div class="grid gap-2.5 sm:grid-cols-[0.65fr_1.35fr_0.8fr]">
    {@render children()}
    <select class="h-12 rounded-xl border border-[#dedee1] bg-white px-3 text-xs font-bold" bind:value={target}>
      {#each targets as option}<option value={option.value}>{option.label}</option>{/each}
    </select>
  </div>
</article>
