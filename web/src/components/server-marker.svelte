<script lang="ts">
  let { emoji }: { emoji?: string } = $props();
</script>

<span class="country-code" aria-hidden="true">{emoji || "VPN"}</span>
