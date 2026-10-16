<script lang="ts">
  import { loadPrefs } from './lib/prefs';
  import ThemeSwitch from './lib/ThemeSwitch.svelte';

  const prefs = $state(loadPrefs());
</script>

<div class="shell">
  <header class="bar">
    <span class="name">ScannerPulse</span>
    <span class="spacer"></span>
    <ThemeSwitch bind:theme={prefs.theme} />
    <a class="admin" href="/admin">Admin</a>
  </header>
  <aside class="rail" aria-label="Filters"></aside>
  <main class="list" aria-label="Calls"></main>
  <footer class="player" aria-label="Player"></footer>
</div>

<style>
  .shell {
    display: grid; min-height: 100vh;
    grid-template: 'bar bar' auto 'rail list' 1fr 'player player' auto / 15rem 1fr;
  }
  .bar {
    grid-area: bar; display: flex; gap: 0.75rem; align-items: center;
    padding: 0.5rem 1rem; background: var(--bar); color: var(--bar-text);
  }
  .name { font-weight: 700; color: var(--accent-strong); }
  .spacer { flex: 1; }
  .admin { color: var(--bar-text); }
  .rail { grid-area: rail; background: var(--surface); border-right: 1px solid var(--line); }
  .list { grid-area: list; min-width: 0; }
  .player { grid-area: player; background: var(--bar); color: var(--bar-text); }
  @media (max-width: 760px) {
    .shell { grid-template: 'bar' auto 'list' 1fr 'player' auto / 1fr; }
    .rail { display: none; }
  }
</style>
