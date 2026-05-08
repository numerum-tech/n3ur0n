<script lang="ts">
  import { transport } from '$lib/transport';

  let status = $state<string>('unknown');

  async function checkHealth() {
    try {
      const res = await fetch('/api/v0/health');
      const json = await res.json();
      status = json.status ?? 'unknown';
    } catch (e) {
      status = `error: ${e}`;
    }
  }
</script>

<section class="space-y-4">
  <p class="text-zinc-400">
    Pre-implementation scaffold. UI surfaces (chat, peers, capabilities, subscriptions, config, logs)
    will land progressively as the core crates fill in.
  </p>

  <button
    class="rounded border border-zinc-700 px-3 py-1 hover:bg-zinc-800"
    onclick={checkHealth}
  >
    Check server health
  </button>

  <p>Status: <span class="font-mono">{status}</span></p>

  <p class="text-xs text-zinc-500">transport: {transport.constructor.name}</p>
</section>
