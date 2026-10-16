<script lang="ts">
  import { untrack } from 'svelte';
  import { SvelteSet } from 'svelte/reactivity';
  import { fetchExcel, type Call, type FilterChoices } from './api';
  import CallList from './CallList.svelte';
  import { queryOf, type Ask, type List } from './calls.svelte';
  import type { ColumnId } from './columns';
  import { exportQuery, saveFile } from './exporting';
  import { callTime } from './format';
  import { FIRST_PAGE, pageCount } from './paging';
  import { player } from './playing.svelte';
  import { PAGE_SIZES } from './prefs';
  import { searchAsk, type SearchFields } from './search';

  let {
    open = $bindable(),
    text = $bindable(),
    results,
    choices,
    onedit,
    ondownload,
    onshare,
    tell,
  }: {
    open: boolean;
    /** The words searched for; a link may bring them. */
    text: string;
    /** The panel's list; the page keeps a text saved or heard live in it. */
    results: List;
    choices: FilterChoices;
    onedit: (call: Call) => void;
    ondownload: (call: Call) => void;
    onshare: (call: Call) => void;
    tell: (text: string) => void;
  } = $props();

  // The panel's own fields, pages and player, over its own list (`results`). It
  // never reads or changes the live list's filters.
  const fields = $state<SearchFields>({ text: '', talkgroup: null, system: null, from: '', to: '', size: 25 });
  const COLUMNS: ColumnId[] = ['time', 'talkgroup', 'length', 'text'];
  const played = new SvelteSet<number>();
  let playing = $state<Call | null>(null);
  /** What the results on screen were asked with: pages and the Excel file follow it, not fields edited since. */
  let searched = $state.raw<Ask | null>(null);
  let exporting = $state(false);

  let dialog: HTMLDialogElement;
  let field: HTMLInputElement;
  let audio: HTMLAudioElement;
  let opener: Element | null = null;

  const ask = () => searchAsk(fields);
  const count = $derived(results.view.total === null || !searched ? null : pageCount(results.view.total, results.view.capped, searched.size));
  const hasNext = $derived(results.view.paging.cursors.length > results.view.paging.index + 1);

  /** A new search is a new list: the first page, counted exactly. */
  function run(event?: SubmitEvent) {
    event?.preventDefault();
    text = fields.text;
    searched = ask();
    void results.show(searched, FIRST_PAGE);
  }

  function turn(index: number) {
    if (!searched) return;
    if (index < results.view.paging.cursors.length) void results.show(searched, { ...results.view.paging, index }, false);
    else void results.goTo(searched, index);
  }

  // Opening shows the panel (not modal: the live page stays usable) and puts
  // the focus in the words; a link's words are searched at once.
  $effect(() => {
    if (open && !dialog.open) {
      opener = document.activeElement;
      dialog.show();
      field.focus();
    } else if (!open && dialog.open) {
      dialog.close();
    }
  });
  // Words given from outside (the bar's search box, or a link) are searched
  // at once, whether the panel was open already or not.
  // Only `open` and `text` are followed: the panel's own typing changes
  // `fields.text`, which must not start a search or put the old words back.
  $effect(() => {
    const words = open ? text : '';
    if (!words) return;
    untrack(() => {
      if (words === fields.text) return;
      fields.text = words;
      run();
    });
  });

  function close() {
    open = false;
    stop();
    if (opener instanceof HTMLElement) opener.focus();
  }

  function key(event: KeyboardEvent) {
    if (event.key === 'Escape') {
      event.stopPropagation();
      close();
    }
  }

  // A result plays in the panel's own player and holds the live one, which
  // goes on when the result ends or the panel closes. Pausing a result by
  // hand keeps the hold.
  function play(call: Call) {
    playing = call;
    played.add(call.id);
    audio.src = `/audio/${call.id}`;
    audio.play().catch(() => {});
  }
  function stop() {
    audio?.pause();
    playing = null;
    player.held = false;
  }
  function failed() {
    if (!audio.error) return;
    tell('That call could not be played.');
    stop();
  }
  // Starting the live player by hand during a hold stops the result.
  let taken = player.takenBack;
  $effect(() => {
    if (player.takenBack !== taken) {
      taken = player.takenBack;
      audio.pause();
      playing = null;
    }
  });

  async function excel() {
    if (!searched) return;
    exporting = true;
    tell('Making the Excel file of every match…');
    try {
      const file = await fetchExcel(exportQuery(queryOf(searched, null, false)));
      saveFile(file.blob, file.name);
      tell(file.note ?? `Saved ${file.name}.`);
    } catch (error) {
      tell(`The Excel file could not be made: ${error instanceof Error ? error.message : String(error)}`);
    } finally {
      exporting = false;
    }
  }
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<dialog bind:this={dialog} class="panel" aria-labelledby="search-title" onkeydown={key}>
  <div class="head">
    <h2 id="search-title">Search the archive</h2>
    <button type="button" class="close" onclick={close}>Close</button>
  </div>
  <form role="search" aria-label="Search the archive" onsubmit={run}>
    <label class="words">
      Words
      <input type="search" bind:this={field} bind:value={fields.text} />
    </label>
    <label>
      Talkgroup
      <select value={fields.talkgroup ?? ''} onchange={(e) => (fields.talkgroup = e.currentTarget.value ? Number(e.currentTarget.value) : null)}>
        <option value="">Any talkgroup</option>
        {#each choices.talkgroup_groups as group (group.system_id)}
          <optgroup label={group.system}>
            {#each group.talkgroups as talkgroup (talkgroup.id)}<option value={talkgroup.id}>{talkgroup.name}</option>{/each}
          </optgroup>
        {/each}
      </select>
    </label>
    <label>
      System
      <select value={fields.system ?? ''} onchange={(e) => (fields.system = e.currentTarget.value ? Number(e.currentTarget.value) : null)}>
        <option value="">Every system</option>
        {#each choices.systems as system (system.id)}<option value={system.id}>{system.name}</option>{/each}
      </select>
    </label>
    <label>From <input type="datetime-local" bind:value={fields.from} /></label>
    <label>To <input type="datetime-local" bind:value={fields.to} /></label>
    <button type="submit" class="go">Search</button>
  </form>
  <p class="hint">Words: “a phrase”, OR, -word, prefix*. With no dates, every date is searched.</p>

  {#if searched}
    <div class="summary">
      <span aria-live="polite">
        {#if results.view.loading && results.view.total === null}Searching…
        {:else if results.view.total !== null}{results.view.total.toLocaleString('en-US')}{results.view.capped ? '+' : ''} {results.view.total === 1 ? 'match' : 'matches'}{/if}
      </span>
      <button type="button" disabled={exporting || !results.view.calls.length} onclick={excel}>Excel — every match</button>
    </div>
    {#if results.view.error}<p class="error" role="alert">{results.view.error}</p>{/if}
    <div class="results">
      <CallList calls={results.view.calls} playingId={playing?.id ?? null} {played} showAll={false} columns={COLUMNS}
        onplay={play} {onedit} {ondownload} {onshare} />
    </div>
    <nav class="pages" aria-label="Result pages">
      <button type="button" onclick={() => turn(results.view.paging.index - 1)} disabled={results.view.paging.index === 0}>Previous</button>
      <span>Page {results.view.paging.index + 1}{count ? ` of ${count.pages.toLocaleString('en-US')}${count.more ? '+' : ''}` : ''}</span>
      <button type="button" onclick={() => turn(results.view.paging.index + 1)} disabled={!hasNext}>Next</button>
      <label>Rows
        <select bind:value={fields.size} onchange={() => run()}>
          {#each PAGE_SIZES as choice (choice)}<option value={choice}>{choice}</option>{/each}
        </select>
      </label>
    </nav>
  {/if}

  <div class="player">
    <audio bind:this={audio} controls preload="none" aria-label="Search result player"
      onplay={() => (player.held = true)} onended={stop} onerror={failed}></audio>
    <p class="now" aria-live="polite">
      {#if playing}<b>{playing.talkgroup_name ?? playing.talkgroup}</b> {callTime(playing.started_at, new Date(0))}{:else}No result playing{/if}
    </p>
  </div>
</dialog>

<style>
  .panel {
    position: fixed; inset: 4rem 1rem 5.5rem auto; margin: 0; z-index: 20; width: min(56rem, calc(100vw - 2rem));
    /* The browser's own dialog style sizes it to its content: the insets decide here. */
    height: auto; max-height: none; display: none; flex-direction: column; gap: 0.5rem; padding: 0.9rem;
    border: 1px solid var(--field-border); border-radius: 8px; background: var(--bg); color: var(--text);
    box-shadow: 0 10px 30px rgb(0 0 0 / 0.35); overflow: hidden;
  }
  .panel[open] { display: flex; }
  .head { display: flex; align-items: center; gap: 0.5rem; }
  h2 { margin: 0; font-size: 1.1rem; flex: 1; }
  button, select, input {
    min-height: 2.25rem; padding: 0.2rem 0.6rem; border: 1px solid var(--field-border); border-radius: 4px;
    background: var(--bg); color: inherit; font: inherit;
  }
  button { cursor: pointer; }
  button:disabled { opacity: 0.55; cursor: default; }
  form { display: flex; flex-wrap: wrap; gap: 0.5rem 0.75rem; align-items: end; }
  form label { display: flex; flex-direction: column; gap: 0.2rem; font-size: 0.85rem; color: var(--muted); }
  form label input, form label select { color: var(--text); font-size: 1rem; }
  .words { flex: 1 1 14rem; }
  .go { background: var(--accent); border-color: var(--accent); color: var(--bg); font-weight: 600; }
  .hint { margin: 0; font-size: 0.85rem; color: var(--muted); }
  .summary, .pages { display: flex; flex-wrap: wrap; gap: 0.5rem; align-items: center; }
  .summary span { margin-right: auto; }
  .pages label { display: flex; gap: 0.35rem; align-items: center; margin-left: auto; }
  .results { flex: 1; min-height: 6rem; overflow: auto; border: 1px solid var(--line); border-radius: 4px; }
  .error { margin: 0; background: var(--warn-bg); color: var(--warn-text); padding: 0.4rem 0.6rem; border-radius: 4px; }
  .player { display: flex; flex-wrap: wrap; gap: 0.5rem 1rem; align-items: center; border-top: 1px solid var(--line); padding-top: 0.5rem; }
  audio { height: 2.25rem; flex: 1 1 14rem; }
  .now { margin: 0; }
  @media (max-width: 760px) {
    .panel { inset: 0; width: auto; border-radius: 0; }
  }
</style>
