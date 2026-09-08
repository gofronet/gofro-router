<script lang="ts">
  import Eye from "lucide-svelte/icons/eye";
  import EyeOff from "lucide-svelte/icons/eye-off";
  import type { HTMLInputAttributes } from "svelte/elements";

  const uid = $props.id();
  let { id = uid, label, value = $bindable(""), class: className = "", ...attributes }: Omit<HTMLInputAttributes, "type" | "value"> & { label: string; value?: string } = $props();
  let visible = $state(false);
</script>

<label class="field" for={id}>{label}
  <span class="password-wrap">
    <input {...attributes} {id} bind:value type={visible ? "text" : "password"} class={className} />
    <button class="icon-btn" type="button" aria-label={`${visible ? "Скрыть" : "Показать"} пароль: ${label}`} aria-controls={id} aria-pressed={visible} disabled={attributes.disabled} onclick={() => visible = !visible}>
      {#if visible}<EyeOff class="icon" />{:else}<Eye class="icon" />{/if}
    </button>
  </span>
</label>
