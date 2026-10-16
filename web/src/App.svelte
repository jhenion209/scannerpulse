<script lang="ts">
  import { onMount } from 'svelte';
  import { choices, startChoices } from './lib/choices.svelte';
  import FilterRail from './lib/FilterRail.svelte';
  import { filtersToSearch, parseFilters } from './lib/filters';
  import { loadPrefs, savePref } from './lib/prefs';
  import ThemeSwitch from './lib/ThemeSwitch.svelte';

  const prefs = $state(loadPrefs());
  const fromUrl = new URLSearchParams(location.search);
  // SmartSort is a preference and a filter: a link that names it wins.
  let filters = $state({ ...parseFilters(location.search), smart: fromUrl.has('smart') ? fromUrl.get('smart') === '1' : prefs.smartSort });
  let paused = $state(false);
  let menuOpen = $state(false);
  const narrow = matchMedia('(max-width: 760px)');

  onMount(startChoices);

  $effect(() => {
    const search = filtersToSearch(filters);
    if (search !== location.search) history.replaceState(null, '', `${location.pathname}${search}${location.hash}`);
    if (filters.smart !== prefs.smartSort) {
      prefs.smartSort = filters.smart;
      savePref('smartSort', filters.smart);
    }
  });

  function toggleRail() {
    if (narrow.matches) menuOpen = !menuOpen;
    else {
      prefs.railOpen = !prefs.railOpen;
      savePref('railOpen', prefs.railOpen);
    }
  }
</script>

<div class="shell" class:rail-closed={!prefs.railOpen} class:menu-open={menuOpen}>
  <header class="bar">
    <button type="button" class="menu" onclick={toggleRail} aria-controls="filters" aria-expanded={narrow.matches ? menuOpen : prefs.railOpen}>
      Filters
    </button>
    <span class="name">ScannerPulse</span>
    <span class="spacer"></span>
    <ThemeSwitch bind:theme={prefs.theme} />
    <a class="admin" href="/admin">Admin</a>
  </header>
  <aside class="rail" id="filters" aria-label="Filters">
    {#if choices.error}<p class="error" role="alert">The filter choices could not be read: {choices.error}</p>{/if}
    <FilterRail bind:filters bind:paused choices={choices.filters} favorites={choices.favorites} />
  </aside>
  <main class="list" aria-label="Calls"></main>
  <footer class="player" aria-label="Player"></footer>
</div>

<style>
  .shell {
    display: grid; min-height: 100vh;
    grid-template: 'bar bar' auto 'rail list' 1fr 'player player' auto / 16rem 1fr;
  }
  .shell.rail-closed { grid-template-columns: 0 1fr; }
  .shell.rail-closed .rail { display: none; }
  .bar {
    grid-area: bar; display: flex; gap: 0.75rem; align-items: center;
    padding: 0.5rem 1rem; background: var(--bar); color: var(--bar-text);
  }
  .menu {
    background: none; color: var(--bar-text); border: 1px solid currentColor; border-radius: 4px;
    padding: 0.25rem 0.6rem; min-height: 2.25rem; cursor: pointer;
  }
  .name { font-weight: 700; color: var(--accent-strong); }
  .spacer { flex: 1; }
  .admin { color: var(--bar-text); }
  .rail { grid-area: rail; background: var(--surface); border-right: 1px solid var(--line); overflow: auto; }
  .error { margin: 0.9rem 0.9rem 0; background: var(--warn-bg); color: var(--warn-text); padding: 0.4rem 0.6rem; border-radius: 4px; }
  .list { grid-area: list; min-width: 0; }
  .player { grid-area: player; background: var(--bar); color: var(--bar-text); }
  @media (max-width: 760px) {
    .shell, .shell.rail-closed { grid-template: 'bar' auto 'list' 1fr 'player' auto / 1fr; }
    .rail, .shell.rail-closed .rail { display: none; }
    .shell.menu-open .rail {
      display: block; position: fixed; inset: 3.25rem 0 0 0; z-index: 10; border-right: 0;
    }
  }
</style>
