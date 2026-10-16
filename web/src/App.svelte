<script lang="ts">
  import { onMount, tick, untrack } from 'svelte';
  import { MediaQuery } from 'svelte/reactivity';
  import type { Call } from './lib/api';
  import CallList from './lib/CallList.svelte';
  import { goTo, show, view } from './lib/calls.svelte';
  import { choices, startChoices } from './lib/choices.svelte';
  import { follow } from './lib/feed';
  import FilterRail from './lib/FilterRail.svelte';
  import { addCall, liveApplies, liveUrl, newestId, setTranscript, wanted, type LiveEvent } from './lib/live';
  import { favoriteUnresolved, filtersToSearch, parseFilters, parsePage, talkgroupsAskedFor } from './lib/filters';
  import Pager from './lib/Pager.svelte';
  import Player from './lib/Player.svelte';
  import { play, played, player } from './lib/playing.svelte';
  import { FIRST_PAGE } from './lib/paging';
  import { loadPrefs, savePref } from './lib/prefs';
  import ThemeSwitch from './lib/ThemeSwitch.svelte';

  const prefs = $state(loadPrefs());
  const fromUrl = new URLSearchParams(location.search);
  // SmartSort is a preference and a filter: a link that names it wins.
  let filters = $state({ ...parseFilters(location.search), smart: fromUrl.has('smart') ? fromUrl.get('smart') === '1' : prefs.smartSort });
  let paused = $state(false);
  let menuOpen = $state(false);
  const narrow = new MediaQuery('max-width: 760px');

  let smartSeen = untrack(() => filters.smart);
  // The page a link names, until the first read has carried the list there.
  // It is the link's only while the filters and page size are the link's.
  let landing = $state(parsePage(location.search));
  const viewKey = () => `${filtersToSearch(filters)}|${prefs.pageSize}`;
  const landingKey = untrack(viewKey);
  const landingPage = () => (viewKey() === landingKey ? landing : 0);

  onMount(startChoices);

  $effect(() => {
    const search = filtersToSearch(filters, view.paging.index || landingPage());
    if (search !== location.search) history.replaceState(null, '', `${location.pathname}${search}${location.hash}`);
    // Only a change made after the page opened is saved: a link's value is
    // for this view alone.
    if (filters.smart !== smartSeen) {
      smartSeen = filters.smart;
      prefs.smartSort = filters.smart;
      savePref('smartSort', filters.smart);
    }
  });

  const ask = () => ({ filters, favorites: choices.favorites, size: prefs.pageSize });

  // A new filter or page size reads the first page again. The favorites are
  // read again every 5 minutes: only a change in the talkgroups they ask for
  // counts, or the list would jump back to page 1 on the timer.
  const listKey = $derived(
    `${filtersToSearch(filters)}|${talkgroupsAskedFor(filters, choices.favorites)}|${prefs.pageSize}`,
  );
  let listEl: HTMLElement;
  let lastId: number | null = null;
  let stopFeed: (() => void) | null = null;
  const asked = () => talkgroupsAskedFor(filters, choices.favorites);
  const systemName = () => choices.filters.systems.find((s) => s.id === filters.system)?.name ?? null;

  // The feed opens after the first page is read, saying the newest call on
  // it, so no call falls between the read and the feed.
  async function readFirstPage() {
    const applied = await show(ask(), FIRST_PAGE);
    // A read that was superseded opens nothing and leaves the link's page
    // to the newer one (the favorites arriving replace the first read).
    if (!applied) return;
    const target = landingPage();
    lastId = newestId(view.calls, null);
    stopFeed?.();
    stopFeed = follow(() => liveUrl(location, asked(), lastId), onLive);
    // The page stays in the address while it is walked to, and is dropped
    // when the walk ends or fails.
    // A walk that a newer read replaced leaves the page to that read.
    if (target > 0 && !(await goTo(ask(), target))) return;
    landing = 0;
  }

  $effect(() => {
    void listKey;
    untrack(readFirstPage);
  });
  onMount(() => () => {
    stopFeed?.();
    clearTimeout(reread);
  });

  // Leaving "Pause updates" reads the first page again. "All dates" is not
  // tracked here: changing it reads the list anew by itself.
  let wasPaused = false;
  $effect(() => {
    const now = paused;
    untrack(() => {
      if (wasPaused && !now && view.paging.index === 0) void show(ask(), view.paging);
    });
    wasPaused = now;
  });

  /** Changes the rows, keeping the rows the listener is reading where they are. */
  async function keepScroll(change: () => void) {
    const before = listEl.scrollHeight;
    change();
    await tick();
    if (listEl.scrollTop > 0) listEl.scrollTop += listEl.scrollHeight - before;
  }

  /** "Play new calls": once the page holds the call, play it if the player is free. */
  function playNew(call: Call | null) {
    const row = call && view.calls.find((c) => c.id === call.id);
    if (row && prefs.autoplayNew && !player.busy) play(row);
  }

  // SmartSort orders a page on the server: new calls read the page again,
  // at most once a second. A read already waiting is left alone, so a steady
  // stream of calls cannot hold the read back; it plays the newest call.
  let reread: ReturnType<typeof setTimeout> | undefined;
  let fresh: Call | null = null;
  const readAgain = (call: Call) => {
    fresh = call;
    if (reread !== undefined) return;
    reread = setTimeout(async () => {
      reread = undefined;
      const newest = fresh;
      fresh = null;
      await show(ask(), view.paging);
      playNew(newest);
    }, 1000);
  };

  function onLive(event: LiveEvent) {
    if (event.type === 'resync') return void show(ask(), view.paging);
    if (event.type === 'transcript') {
      // Text arrives even while paused: it changes a row, not the list.
      const rows = setTranscript(view.calls, event);
      if (rows) view.calls = rows;
      return;
    }
    lastId = newestId([event.call], lastId);
    if (favoriteUnresolved(filters, choices.favorites)) return;
    // A system that cannot be named yet (choices unread, or an unknown id)
    // would let every system's calls in.
    if (filters.system !== null && systemName() === null) return;
    if (!liveApplies(filters, view.paging.index, paused, new Date())) return;
    if (!wanted(event.call, asked(), systemName())) return;
    if (filters.smart) return readAgain(event.call);
    const rows = addCall(view.calls, event.call, prefs.pageSize);
    if (!rows) return;
    // The page grows until it is twice its size, then is read again.
    if (rows.length >= 2 * prefs.pageSize) return void show(ask(), FIRST_PAGE).then(() => playNew(event.call));
    void keepScroll(() => (view.calls = rows));
    if (view.total !== null && !view.capped) view.total += 1;
    playNew(event.call);
  }

  function setSize(size: number) {
    prefs.pageSize = size;
    savePref('pageSize', size);
  }

  function setShowAll(on: boolean) {
    prefs.showAllRows = on;
    savePref('showAllRows', on);
  }

  function go(page: number) {
    if (page === view.paging.index) return;
    if (page < view.paging.cursors.length) void show(ask(), { ...view.paging, index: page }, false);
    else void goTo(ask(), page);
  }

  function setContinuous(on: boolean) {
    prefs.continuous = on;
    savePref('continuous', on);
  }

  function setAutoplayNew(on: boolean) {
    prefs.autoplayNew = on;
    savePref('autoplayNew', on);
  }

  function toggleRail() {
    if (narrow.current) menuOpen = !menuOpen;
    else {
      prefs.railOpen = !prefs.railOpen;
      savePref('railOpen', prefs.railOpen);
    }
  }
</script>

<div class="shell" class:rail-closed={!prefs.railOpen} class:menu-open={menuOpen}>
  <header class="bar">
    <button type="button" class="menu" onclick={toggleRail} aria-controls="filters" aria-expanded={narrow.current ? menuOpen : prefs.railOpen}>
      Filters
    </button>
    <span class="name">ScannerPulse</span>
    <span class="spacer"></span>
    <ThemeSwitch bind:theme={prefs.theme} />
    <a class="admin" href="/admin">Admin</a>
  </header>
  <aside class="rail" id="filters" aria-label="Filters">
    <FilterRail bind:filters bind:paused choices={choices.filters} favorites={choices.favorites} />
  </aside>
  <main class="list" aria-label="Calls" aria-busy={view.loading} bind:this={listEl}>
    <Pager index={view.paging.index} hasNext={view.paging.cursors.length > view.paging.index + 1}
      total={view.total} capped={view.capped} bind:size={() => prefs.pageSize, setSize}
      bind:showAll={() => prefs.showAllRows, setShowAll} ongo={go} />
    {#if choices.error}<p class="error" role="alert">The filter choices could not be read: {choices.error}</p>{/if}
    {#if view.error}<p class="error" role="alert">{view.error}</p>{/if}
    {#if !view.loading && !view.error && view.calls.length === 0}
      <p class="empty">No calls match these filters{filters.allDates || filters.from || filters.to ? '' : ' in the last 24 hours'}.</p>
    {/if}
    <CallList calls={view.calls} playingId={player.call?.id ?? null} {played} showAll={prefs.showAllRows} onplay={(call) => play(call)} />
  </main>
  <footer class="player" aria-label="Player">
    <Player bind:continuous={() => prefs.continuous, setContinuous} bind:autoplayNew={() => prefs.autoplayNew, setAutoplayNew} />
  </footer>
</div>

<style>
  .shell {
    display: grid; height: 100vh; height: 100dvh;
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
  .list { grid-area: list; min-width: 0; overflow: auto; overflow-anchor: none; }
  .empty { margin: 1rem; color: var(--muted); }
  .player { grid-area: player; background: var(--bar); color: var(--bar-text); }
  @media (max-width: 760px) {
    .shell, .shell.rail-closed { grid-template: 'bar' auto 'list' 1fr 'player' auto / 1fr; }
    .rail, .shell.rail-closed .rail { display: none; }
    .shell.menu-open .rail {
      display: block; position: fixed; inset: 3.25rem 0 0 0; z-index: 10; border-right: 0;
    }
  }
</style>
