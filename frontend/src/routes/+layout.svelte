<script lang="ts">
  import '../app.css';
  let { children } = $props();
</script>

<div class="min-h-screen bg-zinc-950 text-zinc-100">
  <header class="border-b border-zinc-800 px-6 py-3">
    <h1 class="text-lg font-semibold tracking-tight">N3UR0N</h1>
  </header>
  <main class="p-6">
    {@render children()}
  </main>
</div>
