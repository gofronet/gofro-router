<script lang="ts">
  import Eye from "lucide-svelte/icons/eye";
  import EyeOff from "lucide-svelte/icons/eye-off";
  import type { HTMLInputAttributes } from "svelte/elements";

  const uid = $props.id();
  let {
    id = uid,
    label,
    value = $bindable(""),
    ...attributes
  }: Omit<HTMLInputAttributes, "type" | "value"> & {
    label: string;
    value?: string;
  } = $props();
  let visible = $state(false);
</script>

<div>
  <label class="mb-1.5 block text-xs font-semibold text-[#74747d]" for={id}>{label}</label>
  <div class="relative">
    <input
      {...attributes}
      {id}
      bind:value
      type={visible ? "text" : "password"}
      class="h-12 w-full min-w-0 rounded-2xl border border-[#dedee1] bg-white pl-4 pr-12 text-base focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[#09090b]"
    />
    <button
      class="absolute inset-y-0 right-0 grid w-12 place-items-center rounded-r-2xl text-[#74747d] hover:text-[#09090b] focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[#09090b]"
      type="button"
      aria-label={`${visible ? "Скрыть" : "Показать"} пароль: ${label}`}
      aria-controls={id}
      aria-pressed={visible}
      disabled={attributes.disabled}
      onclick={() => visible = !visible}
    >
      {#if visible}<EyeOff size={19} />{:else}<Eye size={19} />{/if}
    </button>
  </div>
</div>
