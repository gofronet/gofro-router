<script lang="ts">
  import GitFork from "lucide-svelte/icons/git-fork";
  import Plus from "lucide-svelte/icons/plus";
  import Search from "lucide-svelte/icons/search";
  import ShieldCheck from "lucide-svelte/icons/shield-check";

  import RoutingRule from "../components/routing-rule.svelte";
  import { getAppContext } from "../app-context";
  import type {
    DomainRule,
    IpRule,
    RouteTarget,
    RoutingConfig,
    RoutingTest,
  } from "../domain/models";

  const app = getAppContext();
  const status = $derived(app.status);
  const busy = $derived(app.busy);
  let draft = $state<RoutingConfig>($state.snapshot(app.status.routing.config));
  let testValue = $state("");
  let testResult = $state<RoutingTest | null>(null);
  let testError = $state("");
  let testing = $state(false);

  const targets: { value: RouteTarget; label: string }[] = [
    { value: "direct", label: "Напрямую" },
    { value: "vpn", label: "VPN" },
    { value: "block", label: "Блокировать" },
  ];

  function addDomain() {
    draft.domain_rules.push({
      name: "Новое доменное правило",
      enabled: true,
      matcher: { type: "suffix", value: "" },
      target: "direct",
    });
  }

  function addIp() {
    draft.ip_rules.push({
      name: "Новое IP-правило",
      enabled: true,
      matcher: { type: "cidr", value: "" },
      target: "direct",
    });
  }

  function setDomainType(index: number, type: DomainRule["matcher"]["type"]) {
    draft.domain_rules[index].matcher = { type, value: "" } as DomainRule["matcher"];
  }

  function setIpType(index: number, type: IpRule["matcher"]["type"]) {
    draft.ip_rules[index].matcher = { type, value: "" } as IpRule["matcher"];
  }

  function move<T>(items: T[], index: number, offset: number) {
    const target = index + offset;
    if (target < 0 || target >= items.length) return;
    [items[index], items[target]] = [items[target], items[index]];
  }

  async function save() {
    if (await app.saveRouting($state.snapshot(draft))) {
      draft = $state.snapshot(app.status.routing.config);
    }
  }

  async function testRoute(event: SubmitEvent) {
    event.preventDefault();
    if (!testValue.trim()) return;
    testing = true;
    testError = "";
    testResult = null;
    try {
      testResult = await app.testRouting(testValue.trim());
    } catch (error) {
      testError = error instanceof Error ? error.message : "Не удалось проверить маршрут";
    } finally {
      testing = false;
    }
  }
</script>

<svelte:head><title>Маршруты · Gofro Router</title></svelte:head>

<section class="grid min-w-0 gap-5 lg:gap-6" aria-labelledby="routing-title">
  <header class="min-w-0 px-0.5 py-2 sm:flex sm:items-end sm:justify-between sm:gap-6">
    <div class="min-w-0">
      <span class="text-xs font-bold tracking-[0.18em] text-[#74747d] uppercase">Split routing</span>
      <h1 class="mt-2 text-[clamp(2.25rem,11vw,3.25rem)] leading-[0.98] font-extrabold tracking-[-0.06em] lg:text-[clamp(3rem,5vw,4.2rem)]" id="routing-title">Маршруты</h1>
      <p class="mt-3.5 max-w-2xl text-base leading-relaxed text-[#74747d]">Российские ресурсы идут напрямую, остальной трафик через выбранный VPN.</p>
    </div>
    <button class="mt-4 min-h-13 shrink-0 rounded-2xl border border-[#09090b] bg-[#09090b] px-6 text-sm font-bold text-white sm:mb-1 sm:mt-0" type="button" disabled={busy} onclick={save}>{app.mutation === "routing" ? "Применяем…" : "Применить"}</button>
  </header>

  <div class="grid gap-3 md:grid-cols-[1.3fr_0.7fr]">
    <article class="rounded-[28px] bg-[linear-gradient(145deg,#202024,#09090b_72%)] p-6 text-white shadow-xl shadow-black/10">
      <div class="flex items-start justify-between gap-4">
        <div><span class="text-xs text-[#aaaab1]">Маршрут по умолчанию</span><h2 class="mt-1.5 text-2xl font-bold tracking-[-0.045em]">Весь остальной трафик</h2></div>
        <div class="grid size-12 shrink-0 place-items-center rounded-2xl bg-white text-[#09090b]"><GitFork size={22} /></div>
      </div>
      <select class="mt-6 h-14 w-full rounded-2xl border border-[#3b3b40] bg-[#252529] px-4 text-sm font-bold text-white" bind:value={draft.default_target}>
        {#each targets as target}<option value={target.value}>{target.label}</option>{/each}
      </select>
    </article>
    <article class="rounded-[28px] border border-[#dedee1] bg-white p-6 shadow-sm">
      <div class="flex items-center gap-3"><div class="grid size-11 place-items-center rounded-2xl bg-[#eef4ec] text-[#365a31]"><ShieldCheck size={21} /></div><div><span class="text-xs text-[#74747d]">Состояние</span><strong class="block text-sm">{status.routing.dns_active ? "FakeDNS активен" : "FakeDNS недоступен"}</strong></div></div>
      <dl class="mt-5 grid grid-cols-2 gap-3 text-xs"><div class="rounded-2xl bg-[#f5f5f5] p-3"><dt class="text-[#74747d]">FakeIP</dt><dd class="mt-1 text-lg font-bold">{status.routing.fake_ips}</dd></div><div class="rounded-2xl bg-[#f5f5f5] p-3"><dt class="text-[#74747d]">Dataplane</dt><dd class="mt-1 text-lg font-bold">{status.routing.dataplane_active && status.routing.geosite_loaded && status.routing.geoip_loaded ? "OK" : "Ошибка"}</dd></div></dl>
    </article>
  </div>

  <section class="grid gap-3" aria-labelledby="domain-rules-title">
    <header class="flex items-end justify-between gap-3 px-1"><div><span class="text-xs font-bold tracking-[0.18em] text-[#74747d] uppercase">Exact, suffix и V2Ray GeoSite</span><h2 class="mt-1.5 text-xl font-bold tracking-[-0.035em]" id="domain-rules-title">Доменные правила</h2></div><button class="flex min-h-11 items-center gap-2 rounded-xl border border-[#dedee1] bg-white px-4 text-xs font-bold" type="button" onclick={addDomain}><Plus size={17} />Добавить</button></header>
    {#if draft.domain_rules.length === 0}<div class="rounded-[24px] border border-dashed border-[#c8c8ce] p-7 text-center text-sm text-[#74747d]">Правил нет</div>{/if}
    {#each draft.domain_rules as rule, index}
      <RoutingRule bind:name={rule.name} bind:enabled={rule.enabled} bind:target={rule.target} {index} last={draft.domain_rules.length - 1} onup={() => move(draft.domain_rules, index, -1)} ondown={() => move(draft.domain_rules, index, 1)} onremove={() => draft.domain_rules.splice(index, 1)}>
        <select class="h-12 rounded-xl border border-[#dedee1] bg-white px-3 text-xs font-semibold" value={rule.matcher.type} onchange={(event) => setDomainType(index, event.currentTarget.value as DomainRule["matcher"]["type"])}><option value="exact">Точный домен</option><option value="suffix">Домен и поддомены</option><option value="geo_site">GeoSite</option></select>
        <input class="h-12 min-w-0 rounded-xl border border-[#dedee1] bg-white px-3 text-sm" bind:value={rule.matcher.value} required placeholder={rule.matcher.type === "geo_site" ? "category-ru" : "example.ru"} />
      </RoutingRule>
    {/each}
  </section>

  <section class="grid gap-3" aria-labelledby="ip-rules-title">
    <header class="flex items-end justify-between gap-3 px-1"><div><span class="text-xs font-bold tracking-[0.18em] text-[#74747d] uppercase">CIDR и V2Ray GeoIP</span><h2 class="mt-1.5 text-xl font-bold tracking-[-0.035em]" id="ip-rules-title">IP-правила</h2></div><button class="flex min-h-11 items-center gap-2 rounded-xl border border-[#dedee1] bg-white px-4 text-xs font-bold" type="button" onclick={addIp}><Plus size={17} />Добавить</button></header>
    {#if draft.ip_rules.length === 0}<div class="rounded-[24px] border border-dashed border-[#c8c8ce] p-7 text-center text-sm text-[#74747d]">Правил нет</div>{/if}
    {#each draft.ip_rules as rule, index}
      <RoutingRule bind:name={rule.name} bind:enabled={rule.enabled} bind:target={rule.target} {index} last={draft.ip_rules.length - 1} onup={() => move(draft.ip_rules, index, -1)} ondown={() => move(draft.ip_rules, index, 1)} onremove={() => draft.ip_rules.splice(index, 1)}>
        <select class="h-12 rounded-xl border border-[#dedee1] bg-white px-3 text-xs font-semibold" value={rule.matcher.type} onchange={(event) => setIpType(index, event.currentTarget.value as IpRule["matcher"]["type"])}><option value="cidr">CIDR</option><option value="geo_ip">GeoIP</option></select>
        <input class="h-12 min-w-0 rounded-xl border border-[#dedee1] bg-white px-3 text-sm" bind:value={rule.matcher.value} required placeholder={rule.matcher.type === "geo_ip" ? "ru" : "203.0.113.0/24"} />
      </RoutingRule>
    {/each}
  </section>

  <article class="rounded-[28px] border border-[#dedee1] bg-white p-5 shadow-sm sm:p-6">
    <header><span class="text-xs font-bold tracking-[0.18em] text-[#74747d] uppercase">Route test</span><h2 class="mt-1.5 text-xl font-bold tracking-[-0.035em]">Проверить правило</h2></header>
    <form class="mt-5 grid gap-3 sm:grid-cols-[1fr_auto]" onsubmit={testRoute}><label class="relative"><Search class="absolute left-4 top-1/2 -translate-y-1/2 text-[#74747d]" size={18} /><input class="h-14 w-full rounded-2xl border border-[#dedee1] bg-white pl-11 pr-4 text-sm" bind:value={testValue} placeholder="vk.com или 5.136.1.1" /></label><button class="min-h-13 rounded-2xl border border-[#09090b] bg-[#09090b] px-6 text-sm font-bold text-white" disabled={testing}>{testing ? "Проверяем…" : "Проверить"}</button></form>
    {#if testResult}<p class="mt-3 rounded-2xl bg-[#f5f5f5] p-4 text-sm"><strong>{testResult.value} → {testResult.target.toUpperCase()}</strong><span class="mt-1 block text-xs text-[#74747d]">{testResult.matched_rule || "Маршрут по умолчанию"}</span></p>{/if}
    {#if testError}<p class="mt-3 rounded-2xl border border-red-200 bg-red-50 p-4 text-xs text-red-700">{testError}</p>{/if}
  </article>
</section>
