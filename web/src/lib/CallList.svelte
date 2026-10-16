<script lang="ts">
  import { SvelteSet } from 'svelte/reactivity';
  import type { Call } from './api';
  import { COLUMNS, EXTRA_COLUMNS, extraText, isExtra, type ColumnId } from './columns';
  import { callTime, lengthPercent, seconds } from './format';
  import RowActions from './RowActions.svelte';
  import { pageState } from './selection';
  import { siteZone } from './status.svelte';

  let {
    calls,
    playingId,
    played,
    showAll,
    columns,
    selecting = false,
    chosen = [],
    onchoose = () => {},
    onchooseall = () => {},
    onplay,
    onedit,
    ondownload,
    onshare,
  }: {
    calls: Call[];
    playingId: number | null;
    played: Set<number>;
    showAll: boolean;
    columns: ColumnId[];
    /** Selection mode: a box on each row and one for the page. */
    selecting?: boolean;
    chosen?: number[];
    onchoose?: (id: number) => void;
    onchooseall?: () => void;
    onplay: (call: Call) => void;
    onedit: (call: Call) => void;
    ondownload: (call: Call) => void;
    onshare: (call: Call) => void;
  } = $props();

  const expanded = new SvelteSet<number>();
  // Read again with each page, so "today" moves on with the list.
  const now = $derived.by(() => {
    void calls;
    return new Date();
  });
  const shown = $derived(COLUMNS.filter((c) => columns.includes(c.id)));
  const has = (id: ColumnId) => columns.includes(id);
  const label = (id: ColumnId) => COLUMNS.find((c) => c.id === id)?.label ?? id;
  // A phone shows the chosen plain-value columns as labelled values under the text.
  const chips = (call: Call) =>
    EXTRA_COLUMNS.filter(has)
      .map((id) => ({ id, text: extraText(call, id) }))
      .filter((c) => c.text !== '');
  const state = $derived(pageState(chosen, calls.map((c) => c.id)));
  const talkgroup = (call: Call) => call.talkgroup_name ?? (call.talkgroup === null ? '' : String(call.talkgroup));
</script>

<table class="calls" class:selecting>
  <thead>
    <tr>
      {#if selecting}
        <th scope="col" class="c-select">
          <input type="checkbox" aria-label="Select every call on this page" checked={state === 'all'}
            bind:indeterminate={() => state === 'some', () => {}} onchange={onchooseall} />
        </th>
      {/if}
      <th scope="col" class="c-play"><span class="visually-hidden">Play</span></th>
      {#each shown as column (column.id)}
        <th scope="col" class={`c-${column.id}`} class:extra={isExtra(column.id)} class:text={column.id === 'text'}>{column.label}</th>
      {/each}
      <th scope="col" class="c-actions"><span class="visually-hidden">Actions</span></th>
    </tr>
  </thead>
  <tbody>
    {#each calls as call (call.id)}
      {@const open = showAll || expanded.has(call.id)}
      {@const isPlayed = played.has(call.id) && call.id !== playingId}
      {@const labels = chips(call)}
      <tr
        class:playing={call.id === playingId}
        class:played={isPlayed}
        class:selected={selecting && chosen.includes(call.id)}
        aria-current={call.id === playingId ? 'true' : undefined}
        data-id={call.id}
      >
        {#if selecting}
          <td class="c-select">
            <input type="checkbox" aria-label={`Select ${talkgroup(call)} at ${callTime(call.started_at, now, siteZone())}`}
              checked={chosen.includes(call.id)} onchange={() => onchoose(call.id)} />
          </td>
        {/if}
        <td class="c-play">
          <button type="button" class="play" onclick={() => onplay(call)}
            aria-label={`Play ${talkgroup(call)} at ${callTime(call.started_at, now, siteZone())}${isPlayed ? ' (played)' : ''}`}>{isPlayed ? '✓' : '▶'}</button>
        </td>
        {#each shown as column (column.id)}
          {#if column.id === 'time'}
            {@const when = callTime(call.started_at, now, siteZone())}
            {@const gap = when.indexOf(' ')}
            <td class="c-time time">{#if gap < 0}{when}{:else}<span class="date">{when.slice(0, gap)}</span> {when.slice(gap + 1)}{/if}</td>
          {:else if column.id === 'talkgroup'}
            <td class="c-talkgroup tg">{talkgroup(call)}</td>
          {:else if column.id === 'length'}
            <td class="c-length len">
              <span class="bar" aria-hidden="true"><i style:width={`${lengthPercent(call.duration_ms)}%`}></i></span>
              <span class="secs">{seconds(call.duration_ms)}</span>
            </td>
          {:else if column.id === 'text'}
            <td class="c-text text"><div class="line">
              {#if call.transcript}
                <span class="snippet" class:open>{call.transcript}</span>
                {#if !showAll}
                  <button type="button" class="more" aria-expanded={open}
                    onclick={() => (open ? expanded.delete(call.id) : expanded.add(call.id))}>{open ? 'Less' : 'More'}</button>
                {/if}
              {:else}
                <span class="none">No text</span>
              {/if}
            </div></td>
          {:else if isExtra(column.id)}
            <td class={`c-${column.id} extra`}>{extraText(call, column.id)}</td>
          {/if}
        {/each}
        <td class="c-actions">
          <RowActions label={`${talkgroup(call)} at ${callTime(call.started_at, now, siteZone())}`}
            onedit={() => onedit(call)} ondownload={() => ondownload(call)} onshare={() => onshare(call)} />
        </td>
        {#if labels.length}
          <td class="chips">
            {#each labels as chip (chip.id)}
              <span class="chip"><span class="chip-name">{label(chip.id)}</span> {chip.text}</span>
            {/each}
          </td>
        {/if}
      </tr>
    {/each}
  </tbody>
</table>

<style>
  .calls { width: 100%; border-collapse: collapse; table-layout: auto; }
  th { position: sticky; top: 0; background: var(--surface); text-align: left; font-size: 0.85rem; color: var(--muted); padding: 0.35rem 0.5rem; }
  td { padding: 0.3rem 0.5rem; border-bottom: 1px solid var(--line); vertical-align: top; }
  tr.playing td { background: var(--playing); }
  tr.played td { color: var(--played); }
  tr.selected td { background: color-mix(in srgb, var(--accent) 8%, var(--bg)); }
  .c-select input { width: 1.15rem; height: 1.15rem; margin: 0.4rem 0.1rem; }
  .play { background: none; border: 1px solid var(--line); border-radius: 50%; width: 2rem; height: 2rem; cursor: pointer; color: var(--accent); }
  .time, .len { white-space: nowrap; font-variant-numeric: tabular-nums; }
  .date { white-space: nowrap; }
  .tg { font-weight: 600; }
  .tg, td.extra { white-space: nowrap; }
  .chips { display: none; }
  .bar { display: inline-block; width: 3.5rem; height: 0.4rem; border-radius: 3px; background: var(--line); vertical-align: middle; }
  .bar i { display: block; height: 100%; border-radius: 3px; background: var(--accent); }
  .secs { margin-left: 0.35rem; font-size: 0.85rem; color: var(--muted); }
  .text { width: 100%; }
  .line { display: flex; gap: 0.5rem; align-items: baseline; }
  .snippet { flex: 1; min-width: 0; }
  .snippet { display: -webkit-box; -webkit-line-clamp: 1; line-clamp: 1; -webkit-box-orient: vertical; overflow: hidden; white-space: pre-wrap; overflow-wrap: anywhere; }
  .snippet.open { display: block; }
  .more { background: none; border: 0; padding: 0; color: var(--accent); text-decoration: underline; cursor: pointer; }
  .none { color: var(--muted); font-style: italic; }
  /* A phone shows each call on its own lines: play, time, talkgroup and
     length; then the text across the whole width; then any other chosen
     column as a labelled value. Each cell has a fixed grid column, so a
     hidden one leaves its place empty. The time of a call from an earlier
     day wraps at its space, the date above the time (the date itself never
     breaks, for a browser would break it at its hyphens), so that the
     talkgroup keeps the room; a call from today has no space and stays on
     one line. */
  @media (max-width: 760px) {
    thead { position: absolute; width: 1px; height: 1px; overflow: hidden; clip: rect(0 0 0 0); }
    /* While selecting, the heading row shows with the box for the whole
       page alone; the other headings stay hidden. */
    .selecting thead { position: static; width: auto; height: auto; overflow: visible; clip: auto; display: block; }
    .selecting thead tr { grid-template-columns: auto; }
    .selecting th:not(.c-select) { position: absolute; width: 1px; height: 1px; overflow: hidden; clip: rect(0 0 0 0); padding: 0; }
    tbody, tr, td { display: block; }
    tr { display: grid; grid-template-columns: auto min-content 1fr auto auto; align-items: center; border-bottom: 1px solid var(--line); }
    td { border: 0; padding: 0.2rem 0.4rem; }
    td.extra, th.extra { display: none; }
    .c-play { grid-column: 1; }
    .c-time { grid-column: 2; white-space: normal; }
    .c-talkgroup { grid-column: 3; overflow: hidden; text-overflow: ellipsis; }
    .c-length { grid-column: 4; }
    .c-actions { grid-column: 5; }
    /* Selecting puts a box in front of each row: everything moves one place on. */
    .selecting tr { grid-template-columns: auto auto min-content 1fr auto auto; }
    .selecting .c-select { grid-column: 1; }
    .selecting .c-play { grid-column: 2; }
    .selecting .c-time { grid-column: 3; }
    .selecting .c-talkgroup { grid-column: 4; }
    .selecting .c-length { grid-column: 5; }
    .selecting .c-actions { grid-column: 6; }
    .bar { display: none; }
    td.text { grid-column: 1 / -1; padding-top: 0; }
    /* Each cell's line is set, so none falls to a later line. */
    .c-select, .c-play, .c-time, .c-talkgroup, .c-length, .c-actions { grid-row: 1; }
    td.text { grid-row: 2; }
    td.chips { grid-row: 3; }
    td.chips { display: flex; flex-wrap: wrap; gap: 0.1rem 0.9rem; grid-column: 1 / -1; padding-top: 0; font-size: 0.85rem; color: var(--muted); overflow-wrap: anywhere; }
    .chip-name { font-weight: 600; }
  }
</style>
