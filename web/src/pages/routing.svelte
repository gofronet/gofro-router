<script lang="ts">
  import GripVertical from "lucide-svelte/icons/grip-vertical";
  import MoreHorizontal from "lucide-svelte/icons/more-horizontal";
  import Plus from "lucide-svelte/icons/plus";
  import Search from "lucide-svelte/icons/search";
  import Dialog from "../components/dialog.svelte";
  import type { RouteTarget, RoutingConfig, RoutingTest } from "../domain/models";
  import { getAppContext } from "../app-context";
  import { cloneRules, packRules, reorderRules, type DraftRule } from "./routing-rules";

  type Editor = { rule: DraftRule; index: number } | null;

  const app = getAppContext();
  const status = $derived(app.status);
  const routing = $derived(status.routing.config);
  const targets: { value: RouteTarget; label: string }[] = [{ value: "vpn", label: "Через VPN" }, { value: "direct", label: "Без VPN" }, { value: "block", label: "Блокировать" }];
  const matcherOptions = {
    domain: [{ value: "suffix", label: "Сайт целиком", field: "Адрес сайта", example: "example.com", help: "Без https:// и /страницы. Например, example.com включает mail.example.com." }, { value: "exact", label: "Только этот адрес", field: "Адрес сайта", example: "mail.example.com", help: "Без https:// и /страницы. Только указанный адрес." }, { value: "geo_site", label: "Список сайтов", field: "Название списка", example: "category-ru", help: "Например, category-ru для российских сайтов." }],
    ip: [{ value: "cidr", label: "IP-адрес или сеть", field: "IP-адрес или сеть", example: "192.0.2.1", help: "Один адрес: 192.0.2.1. Диапазон адресов: 192.0.2.0/24." }, { value: "geo_ip", label: "Страна или список адресов", field: "Код страны или название списка", example: "ru", help: "Например, ru для России, de для Германии." }],
  };

  // Keep an editing draft independent from polling updates.
  let draft = $state<DraftRule[]>([]);
  let mode = $state<RoutingConfig["mode"]>("rules");
  let defaultTarget = $state<RouteTarget>("vpn");
  let initialized = $state(false);
  let query = $state("");
  let filter = $state<RouteTarget | "all">("all");
  let editor = $state<Editor>(null);
  let deleting = $state<DraftRule | null>(null);
  let saving = $state(false);
  let saved = $state("");
  let announcement = $state("");
  let testValue = $state("");
  let testResult = $state<RoutingTest | null>(null);
  let testError = $state("");
  let testing = $state(false);
  let dragging = $state<{ key: string; target: number | null; after: boolean } | null>(null);

  $effect(() => {
    if (!initialized) { draft = cloneRules(routing, key); mode = routing.mode; defaultTarget = routing.default_target; initialized = true; }
  });

  const filtered = $derived(draft.filter((rule) => (filter === "all" || rule.target === filter) && `${rule.name} ${rule.value}`.toLowerCase().includes(query.trim().toLowerCase())));
  const reorderDisabled = $derived(Boolean(query.trim() || filter !== "all"));

  function key() { return crypto.randomUUID(); }
  function config(): RoutingConfig {
    return packRules(mode, defaultTarget, draft);
  }
  async function persist(previous: DraftRule[] = draft) {
    if (saving || app.busy) return false;
    saving = true; saved = "";
    const ok = await app.saveRouting($state.snapshot(config()));
    saving = false;
    if (ok) { saved = "Сохранено"; return true; }
    draft = previous;
    return false;
  }
  async function setMode(next: RoutingConfig["mode"]) {
    if (next === mode || saving || app.busy) return;
    const previous = mode; mode = next;
    if (!(await persist())) mode = previous;
  }
  async function setDefaultTarget(next: string) {
    if (next !== "vpn" && next !== "direct" && next !== "block") return;
    if (next === defaultTarget || saving || app.busy) return;
    const previous = defaultTarget; defaultTarget = next;
    if (!(await persist())) defaultTarget = previous;
  }
  function openNew() { editor = { index: draft.length, rule: { key: key(), kind: "domain", name: "", value: "", matcher: "suffix", target: "vpn", enabled: true } }; }
  function normalize(rule: DraftRule) {
    if (rule.matcher === "exact" || rule.matcher === "suffix") {
      try { const url = new URL(rule.value.includes("://") ? rule.value : `https://${rule.value}`); rule.value = url.hostname.toLowerCase().replace(/\.$/, ""); } catch { /* Backend returns authoritative validation errors. */ }
    }
  }
  function setMatcher(value: string) {
    if (!editor) return;
    if (value === "cidr" || value === "geo_ip") editor.rule = { ...editor.rule, kind: "ip", matcher: value, value: "" };
    else if (value === "exact" || value === "suffix" || value === "geo_site") editor.rule = { ...editor.rule, kind: "domain", matcher: value, value: "" };
  }
  async function saveEditor(event: SubmitEvent) {
    event.preventDefault();
    if (!editor || saving) return;
    const previous = $state.snapshot(draft);
    const rule = $state.snapshot(editor.rule); normalize(rule);
    const next = [...draft];
    const oldIndex = next.findIndex((item) => item.key === rule.key);
    if (oldIndex >= 0) next.splice(oldIndex, 1);
    next.splice(Math.max(0, Math.min(editor.index, next.length)), 0, rule);
    draft = next;
    if (await persist(previous)) editor = null;
  }
  async function toggle(rule: DraftRule) {
    const previous = $state.snapshot(draft); rule.enabled = !rule.enabled; draft = [...draft]; await persist(previous);
  }
  async function remove() {
    if (!deleting || saving) return;
    const previous = $state.snapshot(draft); draft = draft.filter((rule) => rule.key !== deleting?.key);
    if (await persist(previous)) { deleting = null; editor = null; }
  }
  async function move(from: number, to: number, focusKey = draft[from]?.key) {
    if (saving || reorderDisabled || from === to || to < 0 || to >= draft.length) return;
    const previous = $state.snapshot(draft); const rule = draft[from]; draft = reorderRules(draft, from, to);
    if (await persist(previous)) { announcement = `«${rule.name || "Правило"}» перемещено на позицию ${to + 1}.`; requestAnimationFrame(() => document.querySelector<HTMLButtonElement>(`[data-rule-key="${focusKey}"]`)?.focus()); }
  }
  function pointerDown(event: PointerEvent & { currentTarget: HTMLButtonElement }, rule: DraftRule) {
    if (reorderDisabled || saving || event.button !== 0) return;
    dragging = { key: rule.key, target: null, after: false }; event.currentTarget.setPointerCapture(event.pointerId);
  }
  function pointerMove(event: PointerEvent) {
    if (!dragging) return;
    const row = document.elementFromPoint(event.clientX, event.clientY)?.closest<HTMLElement>("[data-rule-row]");
    if (!row || row.dataset.ruleRow === dragging.key) return;
    dragging.target = draft.findIndex((rule) => rule.key === row.dataset.ruleRow);
    dragging.after = event.clientY > row.getBoundingClientRect().top + row.getBoundingClientRect().height / 2;
  }
  function pointerUp() {
    if (!dragging) return;
    const { key, target, after } = dragging; dragging = null;
    const from = draft.findIndex((rule) => rule.key === key); if (target !== null) move(from, target + (after ? 1 : 0) - (from < target ? 1 : 0), key);
  }
  function cancelDrag() {
    if (!dragging) return;
    dragging = null; announcement = "Перемещение отменено.";
  }
  function handleReorderKey(event: KeyboardEvent, rule: DraftRule) {
    if (event.key === "Escape") { dragging = null; announcement = "Перемещение отменено."; return; }
    if (event.key !== "ArrowUp" && event.key !== "ArrowDown") return;
    event.preventDefault(); move(draft.indexOf(rule), draft.indexOf(rule) + (event.key === "ArrowUp" ? -1 : 1), rule.key);
  }
  async function testRoute(event: SubmitEvent) {
    event.preventDefault(); if (!testValue.trim() || testing) return;
    testing = true; testError = ""; testResult = null;
    try { testResult = await app.testRouting(testValue.trim()); } catch (error) { testError = error instanceof Error ? error.message : "Не удалось проверить маршрут"; } finally { testing = false; }
  }
  function closeEditor() { if (!saving) editor = null; }
</script>

<svelte:head><title>Правила VPN · Gofro Router</title></svelte:head>

<section aria-labelledby="routing-rules-title">
  <section class="panel mode-panel">
    <h2>Как использовать VPN</h2>
    <div class="segmented" aria-label="Режим VPN">
      <button type="button" aria-pressed={mode === "rules"} disabled={saving || app.busy} onclick={() => setMode("rules")}>По правилам</button>
      <button type="button" aria-pressed={mode === "all"} disabled={saving || app.busy} onclick={() => setMode("all")}>Весь интернет</button>
    </div>
    <p>{mode === "all" ? "Правила сохранены, но сейчас не применяются." : defaultTarget === "block" ? "Остальные сайты блокируются." : defaultTarget === "vpn" ? "Остальные сайты открываются через VPN." : "Остальные сайты открываются без VPN."}</p>
    {#if !status.vpn_enabled}<p>VPN отключён. Правила блокировки могут оставаться активными.</p>{/if}
  </section>

  <div class="section-caption">
    <div><h2 id="routing-rules-title">Правила <span class="count">{draft.length}</span></h2><p>Если сайту подходят несколько правил, сработает верхнее.</p></div>
    <button class="btn" type="button" disabled={saving || app.busy} onclick={openNew}><Plus class="icon" />Добавить</button>
  </div>
  <div class="form-line mb-[15px]">
    <label class="search-input"><Search class="icon" /><input type="search" bind:value={query} placeholder="Найти сайт или правило" aria-label="Поиск правил" /></label>
    <select class="w-[126px] shrink-0 text-[13px] min-[921px]:w-[145px]" bind:value={filter} aria-label="Фильтр правил"><option value="all">Все правила</option>{#each targets as target (target.value)}<option value={target.value}>{target.label}</option>{/each}</select>
  </div>
  {#if reorderDisabled}<p class="notice">Очистите поиск и фильтр, чтобы изменить порядок.</p>{/if}
  <section class="panel">
    {#each filtered as rule (rule.key)}
      {@const ruleIndex = draft.indexOf(rule)}
      <article class="rule-row" class:disabled={!rule.enabled} class:dragging={dragging?.key === rule.key} class:drop-before={dragging?.target === ruleIndex && !dragging.after} class:drop-after={dragging?.target === ruleIndex && dragging.after} data-rule-row={rule.key}>
        <span class="rule-index">{String(ruleIndex + 1).padStart(2, "0")}</span>
        <button class="drag-handle" data-rule-key={rule.key} type="button" aria-keyshortcuts="ArrowUp ArrowDown" aria-label={`Изменить порядок: ${rule.name}. Стрелки вверх и вниз перемещают правило.`} aria-pressed={dragging?.key === rule.key} disabled={reorderDisabled || saving || app.busy} onpointerdown={(event) => pointerDown(event, rule)} onpointermove={pointerMove} onpointerup={pointerUp} onpointercancel={cancelDrag} onkeydown={(event) => handleReorderKey(event, rule)}><GripVertical class="icon" /></button>
        <button class="switch" type="button" role="switch" aria-checked={rule.enabled} aria-label={`Включить правило ${rule.name}`} disabled={saving || app.busy} onclick={() => toggle(rule)}><span class="switch-track"></span></button>
        <div class="row-main"><h3>{rule.name || "Без названия"}</h3><p>{rule.value || "Значение не указано"}</p></div>
        <span class="tag">{targets.find((target) => target.value === rule.target)?.label}</span>
        <button class="icon-btn" type="button" aria-label={`Изменить ${rule.name}`} disabled={saving || app.busy} onclick={() => editor = { rule: $state.snapshot(rule), index: ruleIndex }}><MoreHorizontal class="icon" /></button>
      </article>
    {:else}<div class="empty"><h3>{draft.length ? "Правила не найдены" : "Правил пока нет"}</h3><p>{draft.length ? "Измените запрос или фильтр." : "Добавьте первое правило."}</p></div>{/each}
  </section>
  {#if saved}<p class="notice" role="status">{saved}</p>{/if}
  <p class="sr-only" aria-live="polite">{announcement}</p>

  <section class="panel mt-5" aria-labelledby="route-test-title">
    <div class="panel-head"><h2 id="route-test-title">Как откроется сайт?</h2></div>
    <div class="panel-body">
      <form onsubmit={testRoute}>
        <label class="field">Адрес сайта или IP<input bind:value={testValue} placeholder="Например, youtube.com" autocapitalize="off" spellcheck="false" required /></label>
        <button class="btn" disabled={testing}>{testing ? "Проверяем…" : "Проверить"}</button>
      </form>
      {#if testResult}{@const result = testResult}<div class="notice" role="status"><strong>{result.value} → {targets.find((target) => target.value === result.target)?.label}</strong><span class="small block mt-1">{result.matched_rule ? `Правило: ${result.matched_rule}` : "Маршрут по умолчанию"}</span>{#if result.scope === "domain_preview"}<span class="small block mt-2">Предварительный результат для домена. IP и LAN-зависимые правила зависят от DNS.</span>{/if}{#if !status.vpn_enabled}<span class="small block mt-2">VPN выключен; правила блокировки всё равно могут применяться.</span>{/if}</div>{/if}
      {#if testError}<p class="error" role="alert">{testError}</p>{/if}
    </div>
  </section>
  <section class="panel">
    <details class="disclosure">
      <summary>Остальные сайты</summary>
      <div class="details-content">
        <label class="field">Как открывать сайты без правила<select value={defaultTarget} disabled={saving || app.busy} onchange={(event) => setDefaultTarget(event.currentTarget.value)}>{#each targets as target (target.value)}<option value={target.value}>{target.label}</option>{/each}</select></label>
      </div>
    </details>
  </section>
</section>

{#if editor}
  {@const activeEditor = editor}
  <Dialog title={draft.some((rule) => rule.key === activeEditor.rule.key) ? "Изменить правило" : "Новое правило"} onclose={closeEditor} busy={saving}>
    <form onsubmit={saveEditor}><label class="field">Название<input bind:value={activeEditor.rule.name} required maxlength="64" placeholder="Например, рабочий сайт" /></label><label class="field">Для чего<select value={activeEditor.rule.matcher} onchange={(event) => setMatcher(event.currentTarget.value)}><option value="suffix">Сайт целиком</option><option value="exact">Только этот адрес</option><option value="geo_site">Список сайтов</option><option value="cidr">IP-адрес или сеть</option><option value="geo_ip">Страна или список адресов</option></select></label><label class="field">Значение<input bind:value={activeEditor.rule.value} required autocapitalize="off" spellcheck="false" placeholder={activeEditor.rule.matcher === "cidr" ? "192.0.2.0/24" : activeEditor.rule.matcher === "geo_ip" ? "ru" : activeEditor.rule.matcher === "geo_site" ? "category-ru" : "example.com"} /><span class="field-help">{matcherOptions[activeEditor.rule.kind].find((item) => item.value === activeEditor.rule.matcher)?.help}</span></label><label class="field">Как открывать<select bind:value={activeEditor.rule.target}>{#each targets as target (target.value)}<option value={target.value}>{target.label}</option>{/each}</select></label><details class="disclosure" style="padding: 0"><summary>Порядок применения</summary><div class="details-content"><label class="field">Позиция в списке<input type="number" min="1" max={draft.length + (draft.some((rule) => rule.key === activeEditor.rule.key) ? 0 : 1)} value={activeEditor.index + 1} onchange={(event) => activeEditor.index = Number(event.currentTarget.value) - 1} /></label></div></details>{#if app.actionError}<p class="error" role="alert">{app.actionError}</p>{/if}<div class="form-actions">{#if draft.some((rule) => rule.key === activeEditor.rule.key)}<button class="btn ghost danger" type="button" disabled={saving} onclick={() => deleting = activeEditor.rule}>Удалить</button>{:else}<span></span>{/if}<button class="btn primary" type="submit" disabled={saving}>{saving ? "Сохраняем…" : "Сохранить"}</button></div></form>
  </Dialog>
{/if}

{#if deleting}
  <Dialog title="Удалить правило?" onclose={() => deleting = null} busy={saving}><p class="dialog-intro">«{deleting.name || "Без названия"}» будет удалено.</p>{#if app.actionError}<p class="error" role="alert">{app.actionError}</p>{/if}<div class="form-actions"><button class="btn ghost" type="button" disabled={saving} onclick={() => deleting = null}>Отмена</button><button class="btn primary danger" type="button" disabled={saving} onclick={remove}>Удалить</button></div></Dialog>
{/if}
