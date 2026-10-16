<script lang="ts">
  import { onMount, tick, untrack } from 'svelte';
  import { MediaQuery } from 'svelte/reactivity';
  import { ApiError, fetchAudio, fetchCall, fetchExcel, type Call, type CallDetail } from './lib/api';
  import { audioBody, csvText, exportName, exportQuery, pageTable, saveFile } from './lib/exporting';
  import ExportMenu from './lib/ExportMenu.svelte';
  import { sharedCallId, shareUrl } from './lib/actions';
  import CallList from './lib/CallList.svelte';
  import { defaultColumns, type ColumnId } from './lib/columns';
  import { createList, goTo, show, view } from './lib/calls.svelte';
  import { choices, startChoices } from './lib/choices.svelte';
  import { follow } from './lib/feed';
  import FilterRail from './lib/FilterRail.svelte';
  import { addCall, liveApplies, liveUrl, newestId, setTranscript, wanted, type LiveEvent } from './lib/live';
  import { favoriteUnresolved, filtersToSearch, listQuery, parseFilters, parsePage, talkgroupsAskedFor } from './lib/filters';
  import EditDialog from './lib/EditDialog.svelte';
  import Pager from './lib/Pager.svelte';
  import ShareDialog from './lib/ShareDialog.svelte';
  import SharedCall from './lib/SharedCall.svelte';
  import SearchPanel from './lib/SearchPanel.svelte';
  import { searchOf, withSearch } from './lib/search';
  import { keepOnPage, toggleAll, toggleOne } from './lib/selection';
  import Player from './lib/Player.svelte';
  import { play, played, player } from './lib/playing.svelte';
  import { FIRST_PAGE } from './lib/paging';
  import { loadPrefs, resetPref, savePref } from './lib/prefs';
  import { noteCall, startStatus, status } from './lib/status.svelte';
  import StatusChips from './lib/StatusChips.svelte';
  import ThemeSwitch from './lib/ThemeSwitch.svelte';

  const prefs = $state(loadPrefs());
  const fromUrl = new URLSearchParams(location.search);
  // SmartSort is a preference and a filter: a link that names it wins.
  let filters = $state({ ...parseFilters(location.search), smart: fromUrl.has('smart') ? fromUrl.get('smart') === '1' : prefs.smartSort });
  let paused = $state(false);
  let menuOpen = $state(false);
  const narrow = new MediaQuery('max-width: 760px');
  // The chosen columns; until one is chosen, today's layout for the screen's width.
  const columns = $derived(prefs.columns ?? defaultColumns(narrow.current));

  let smartSeen = untrack(() => filters.smart);
  // The page a link names, until the first read has carried the list there.
  // It is the link's only while the filters and page size are the link's.
  let landing = $state(parsePage(location.search));
  const viewKey = () => `${filtersToSearch(filters)}|${prefs.pageSize}`;
  const landingKey = untrack(viewKey);
  const landingPage = () => (viewKey() === landingKey ? landing : 0);

  onMount(startChoices);
  onMount(startStatus);

  // The search panel: open when a link names words (`search=`), or from the
  // bar's search box.
  const linkWords = searchOf(location.search) ?? '';
  let searchText = $state(linkWords);
  let searchOpen = $state(linkWords !== '');
  let barText = $state(linkWords);
  // The panel's list lives here, so a text saved or heard live reaches its rows too.
  const found = createList();
  function openSearch(event: SubmitEvent) {
    event.preventDefault();
    if (barText.trim()) searchText = barText.trim();
    searchOpen = true;
  }

  $effect(() => {
    const search = withSearch(filtersToSearch(filters, view.paging.index || landingPage()), searchOpen ? searchText : null);
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
    lastId = newestId(view.calls, null);
    stopFeed?.();
    stopFeed = follow(() => liveUrl(location, asked(), lastId), onLive);
    // While the chosen favorite is unknown this list is a placeholder: the
    // read that follows the favorites arriving carries the link's page.
    if (favoriteUnresolved(filters, choices.favorites)) return;
    const target = landingPage();
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
    // A search result playing holds the live player: new calls wait too.
    if (row && prefs.autoplayNew && !player.busy && !player.held) play(row);
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
      return setText(event);
    }
    lastId = newestId([event.call], lastId);
    noteCall(event.call.started_at);
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

  /** A call's text changed: its row follows in the list and in the search panel's results. */
  function setText(event: Extract<LiveEvent, { type: 'transcript' }>) {
    for (const list of [view, found.view]) {
      const rows = setTranscript(list.calls, event);
      if (rows) list.calls = rows;
    }
  }

  function setSize(size: number) {
    prefs.pageSize = size;
    savePref('pageSize', size);
  }

  function setColumns(chosen: ColumnId[]) {
    prefs.columns = chosen;
    savePref('columns', chosen);
  }

  function resetColumns() {
    prefs.columns = null;
    resetPref('columns');
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

  // Selection mode. While it is on, new calls are held back (as with Pause
  // updates), so the rows do not move under the listener's pointer; leaving
  // it puts Pause back as it was.
  let selecting = $state(false);
  let chosen = $state<number[]>([]);
  let pausedBefore = false;
  function setSelecting(on: boolean) {
    if (on === selecting) return;
    selecting = on;
    if (on) {
      pausedBefore = paused;
      paused = true;
    } else {
      paused = pausedBefore;
      chosen = [];
    }
  }
  // The choice never reaches past the page on screen.
  $effect(() => {
    const page = view.calls.map((c) => c.id);
    untrack(() => {
      const kept = keepOnPage(chosen, page);
      if (kept.length !== chosen.length) chosen = kept;
    });
  });

  // Export. Excel and call audio are built on the server from every match
  // of the list (or, for audio, the chosen calls); CSV, PDF and print are the
  // page on screen, or the chosen calls when some are ticked. A file of the
  // page or the chosen calls is named by those calls' own time range, as the
  // server names a file of chosen calls.
  let exporting = $state(false);
  let printChosen = $state(false);
  const zone = () => status.data?.time_zone ?? 'UTC';
  const listParams = () => listQuery(filters, choices.favorites, { size: prefs.pageSize }, new Date());
  const pageRows = () =>
    selecting && chosen.length ? view.calls.filter((c) => chosen.includes(c.id)) : view.calls;
  const oldestFirst = (calls: Call[]) => [...calls].sort((a, b) => Date.parse(a.started_at) - Date.parse(b.started_at));
  const pageName = (ext: string) => exportName(oldestFirst(pageRows()), null, null, zone(), ext);
  const why = (error: unknown) => (error instanceof Error ? error.message : String(error));

  async function excel() {
    exporting = true;
    tell('Making the Excel file of every match…');
    try {
      const file = await fetchExcel(exportQuery(listParams()));
      saveFile(file.blob, file.name);
      tell(file.note ?? `Saved ${file.name}.`);
    } catch (error) {
      tell(`The Excel file could not be made: ${why(error)}`);
    } finally {
      exporting = false;
    }
  }

  function csv() {
    if (selecting && chosen.length === 0) return tell('Nothing is selected: tick calls first.');
    const name = pageName('csv');
    saveFile(new Blob([csvText(pageTable(pageRows(), columns, zone()))], { type: 'text/csv' }), name);
    tell(`Saved ${name}.`);
  }

  /** Prints the page (or the chosen calls); the file name the browser offers
   * for a PDF is the export name, as the page's title. */
  async function printPage(kind: 'pdf' | 'print') {
    if (selecting && chosen.length === 0) return tell('Nothing is selected: tick calls first.');
    const title = document.title;
    printChosen = selecting && chosen.length > 0;
    document.title = pageName('pdf').replace(/\.pdf$/, '');
    await tick();
    const restore = () => {
      document.title = title;
      printChosen = false;
    };
    window.addEventListener('afterprint', restore, { once: true });
    window.print();
    if (kind === 'pdf') tell('Choose “Save as PDF” in the print dialog to keep a PDF.');
  }

  async function audio() {
    if (selecting && chosen.length === 0) return tell('Nothing is selected: tick calls first.');
    exporting = true;
    const ids = selecting && chosen.length ? chosen : undefined;
    tell(ids ? `Joining the audio of ${ids.length} chosen calls…` : 'Joining the audio of every match…');
    try {
      const file = await fetchAudio(audioBody(listParams(), ids));
      saveFile(file.blob, file.name);
      tell(file.note ?? `Saved ${file.name}.`);
    } catch (error) {
      tell(`The call audio could not be made: ${why(error)}`);
    } finally {
      exporting = false;
    }
  }

  // The actions on one call.
  let editing = $state<Call | null>(null);
  let sharing = $state<string | null>(null);
  let notice = $state<string | null>(null);
  let noticeTimer: ReturnType<typeof setTimeout> | undefined;
  function tell(text: string) {
    notice = text;
    clearTimeout(noticeTimer);
    noticeTimer = setTimeout(() => (notice = null), 6000);
  }

  function saved(id: number, transcript: string, origin: string) {
    setText({ type: 'transcript', id, talkgroup_id: null, transcript, origin });
    if (shared.call?.id === id) shared.call = { ...shared.call, transcript, origin };
    tell('The text is saved.');
  }

  /** Downloads the call under its file's own name. */
  async function download(call: Call) {
    try {
      const detail = await fetchCall(call.id);
      const link = document.createElement('a');
      link.href = `/audio/${call.id}`;
      link.download = detail.file;
      document.body.append(link);
      link.click();
      link.remove();
    } catch (error) {
      tell(`The call could not be downloaded: ${error instanceof Error ? error.message : String(error)}`);
    }
  }

  /** Copies a link to the call; where the browser will not, shows it to copy by hand. */
  async function share(call: Call) {
    const url = shareUrl(location.origin, call.id);
    try {
      await navigator.clipboard.writeText(url);
      tell(`Link copied: ${url}`);
    } catch {
      sharing = url;
    }
  }

  // A link to one call (`/calls/<id>`) shows it above the list.
  const sharedId = sharedCallId(location.pathname);
  const shared = $state<{ open: boolean; call: CallDetail | null; error: string | null }>({
    open: sharedId !== null,
    call: null,
    error: null,
  });
  onMount(() => {
    if (sharedId === null) return;
    fetchCall(sharedId)
      .then((call) => (shared.call = call))
      .catch((error) => {
        shared.error =
          error instanceof ApiError && error.status === 404
            ? 'That call is not in the archive.'
            : `The call could not be read: ${error instanceof Error ? error.message : String(error)}`;
      });
  });
  function closeShared() {
    shared.open = false;
    history.replaceState(null, '', `/${location.search}${location.hash}`);
  }

  function toggleRail() {
    if (narrow.current) menuOpen = !menuOpen;
    else {
      prefs.railOpen = !prefs.railOpen;
      savePref('railOpen', prefs.railOpen);
    }
  }
</script>

<div class="shell" class:rail-closed={!prefs.railOpen} class:menu-open={menuOpen} class:print-chosen={printChosen}>
  <header class="bar">
    <button type="button" class="menu" onclick={toggleRail} aria-controls="filters" aria-expanded={narrow.current ? menuOpen : prefs.railOpen}>
      Filters
    </button>
    <span class="name">ScannerPulse</span>
    <form class="find" role="search" aria-label="Search calls" onsubmit={openSearch}>
      <input type="search" aria-label="Words to search for" placeholder="Search calls" bind:value={barText} />
      <button type="submit">Search</button>
    </form>
    <span class="spacer"></span>
    <StatusChips />
    <ThemeSwitch bind:theme={prefs.theme} />
    <a class="admin" href="/admin">Admin</a>
  </header>
  <!-- Outside the scrolling list, over the search panel: seen wherever the list is. -->
  <p class="notice" role="status">{notice ?? ''}</p>
  <aside class="rail" id="filters" aria-label="Filters">
    <FilterRail bind:filters bind:paused choices={choices.filters} favorites={choices.favorites} />
  </aside>
  <main class="list" aria-label="Calls" aria-busy={view.loading} bind:this={listEl}>
    <Pager index={view.paging.index} hasNext={view.paging.cursors.length > view.paging.index + 1}
      total={view.total} capped={view.capped} bind:size={() => prefs.pageSize, setSize}
      bind:showAll={() => prefs.showAllRows, setShowAll} {columns} customised={prefs.columns !== null}
      oncolumns={setColumns} onresetcolumns={resetColumns} ongo={go}
      bind:selecting={() => selecting, setSelecting} chosenCount={chosen.length}>
      {#snippet extra()}
        <ExportMenu {selecting} chosenCount={selecting ? chosen.length : 0} busy={exporting}
          onexcel={excel} oncsv={csv} onprint={printPage} onaudio={audio} />
      {/snippet}
    </Pager>
    {#if shared.open}
      <SharedCall call={shared.call} error={shared.error} onplay={(call) => play(call)} onclose={closeShared} />
    {/if}
    {#if choices.error}<p class="error" role="alert">The filter choices could not be read: {choices.error}</p>{/if}
    {#if view.error}<p class="error" role="alert">{view.error}</p>{/if}
    {#if !view.loading && !view.error && view.calls.length === 0}
      <p class="empty">No calls match these filters{filters.allDates || filters.from || filters.to ? '' : ' in the last 24 hours'}.</p>
    {/if}
    <CallList calls={view.calls} playingId={player.call?.id ?? null} {played} showAll={prefs.showAllRows} {columns} onplay={(call) => play(call)}
      onedit={(call) => (editing = call)} ondownload={download} onshare={share}
      {selecting} {chosen} onchoose={(id) => (chosen = toggleOne(chosen, id))}
      onchooseall={() => (chosen = toggleAll(chosen, view.calls.map((c) => c.id)))} />
  </main>
  <footer class="player" aria-label="Player">
    <Player bind:continuous={() => prefs.continuous, setContinuous} bind:autoplayNew={() => prefs.autoplayNew, setAutoplayNew} />
  </footer>
</div>
<EditDialog call={editing} onclose={() => (editing = null)} onsaved={saved} />
<ShareDialog url={sharing} onclose={() => (sharing = null)} />
<SearchPanel bind:open={searchOpen} bind:text={searchText} results={found} choices={choices.filters}
  onedit={(call) => (editing = call)} ondownload={download} onshare={share} {tell} />

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
  .bar { flex-wrap: wrap; row-gap: 0.4rem; }
  .find { display: flex; gap: 0.3rem; }
  .find input {
    min-height: 2.25rem; width: 14rem; padding: 0.2rem 0.5rem; border-radius: 4px;
    border: 1px solid var(--bar-text); background: var(--bg); color: var(--text); font: inherit;
  }
  .find button {
    min-height: 2.25rem; padding: 0 0.7rem; border-radius: 4px; border: 1px solid var(--bar-text);
    background: none; color: var(--bar-text); font: inherit; cursor: pointer;
  }
  .spacer { flex: 1; }
  .admin { color: var(--bar-text); }
  .rail { grid-area: rail; background: var(--surface); border-right: 1px solid var(--line); overflow: auto; }
  .error { margin: 0.9rem 0.9rem 0; background: var(--warn-bg); color: var(--warn-text); padding: 0.4rem 0.6rem; border-radius: 4px; }
  .list { grid-area: list; min-width: 0; overflow: auto; overflow-anchor: none; }
  .empty { margin: 1rem; color: var(--muted); }
  /* A line over the top of the list's cell, in front of the search panel. It
     takes no room, so no row moves when it shows, and lets clicks through. */
  .notice {
    grid-area: list; align-self: start; justify-self: center; z-index: 30; margin: 0; padding: 0;
    max-width: calc(100% - 1.8rem); overflow-wrap: anywhere; pointer-events: none;
  }
  .notice:not(:empty) {
    margin-top: 0.6rem; padding: 0.4rem 0.6rem; border-radius: 4px; background: var(--surface);
    border: 1px solid var(--field-border); box-shadow: 0 4px 14px rgb(0 0 0 / 0.25);
  }
  .player { grid-area: player; background: var(--bar); color: var(--bar-text); }
  /* Printing (and a PDF saved from the print dialog) keeps the calls only:
     every row's whole text, without the controls. */
  @media print {
    .bar, .rail, .player, .notice, .error, :global(.pager), :global(.c-actions), :global(.c-select), :global(.c-play),
    :global(.calls .more) { display: none !important; }
    .shell { display: block; height: auto; }
    .list { overflow: visible; }
    :global(.calls .snippet) { display: block !important; }
    .print-chosen :global(tbody tr:not(.selected)) { display: none !important; }
    :global(.calls tr.selected td) { background: none !important; }
  }
  @media (max-width: 760px) {
    /* A phone keeps the Search button; the panel has the words field. */
    .find input { display: none; }
    .shell, .shell.rail-closed { grid-template: 'bar' auto 'list' 1fr 'player' auto / 1fr; }
    .rail, .shell.rail-closed .rail { display: none; }
    .shell.menu-open .rail {
      display: block; position: fixed; inset: 3.25rem 0 0 0; z-index: 10; border-right: 0;
    }
  }
</style>
