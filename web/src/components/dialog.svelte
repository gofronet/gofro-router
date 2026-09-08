<script lang="ts">
  import { onMount, tick } from "svelte";
  import X from "lucide-svelte/icons/x";
  import type { Snippet } from "svelte";

  let { title, onclose, busy = false, children }: { title: string; onclose: () => void; busy?: boolean; children: Snippet } = $props();
  const titleId = $props.id();
  let dialog = $state<HTMLDialogElement>();
  let restoreFocus: Element | null = null;
  let previousTitle: string | undefined;

  function close() { if (!busy) onclose(); }
  function cancel(event: Event) { if (busy) event.preventDefault(); else onclose(); }
  function backdrop(event: MouseEvent) {
    if (event.target !== dialog || !dialog) return;
    const { left, right, top, bottom } = dialog.getBoundingClientRect();
    if (event.clientX < left || event.clientX > right || event.clientY < top || event.clientY > bottom) close();
  }

  onMount(() => {
    restoreFocus = document.activeElement;
    dialog?.showModal();
    void tick().then(() => dialog?.querySelector<HTMLElement>('input:not([type="hidden"]):not(:disabled), textarea:not(:disabled), select:not(:disabled), button:not(:disabled)')?.focus());
    return () => { if (dialog?.open) dialog.close(); if (restoreFocus instanceof HTMLElement && restoreFocus.isConnected) restoreFocus.focus(); };
  });

  $effect(() => {
    const changed = previousTitle !== undefined && title !== previousTitle;
    previousTitle = title;
    if (changed && dialog && (document.activeElement === document.body || document.activeElement === dialog)) {
      void tick().then(() => dialog?.querySelector<HTMLElement>('input:not([type="hidden"]):not(:disabled), textarea:not(:disabled), select:not(:disabled), button:not(:disabled)')?.focus());
    }
  });
</script>

<dialog bind:this={dialog} aria-labelledby={titleId} oncancel={cancel} onclick={backdrop}>
  <div class="dialog-head"><h2 id={titleId}>{title}</h2><button class="icon-btn" type="button" aria-label="Закрыть" disabled={busy} onclick={close}><X class="icon" /></button></div>
  <div class="dialog-body">{@render children()}</div>
</dialog>
