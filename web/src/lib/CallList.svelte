<script lang="ts">
  import { SvelteSet } from 'svelte/reactivity';
  import type { Call } from './api';
  import { COLUMNS, EXTRA_COLUMNS, extraText, isExtra, type ColumnId } from './columns';
  import { callTime, lengthPercent, seconds } from './format';
  import RowActions from './RowActions.svelte';

  let {
    calls,
    playingId,
    played,
    showAll,
    columns,
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
  const talkgroup = (call: Call) => call.talkgroup_name ?? (call.talkgroup === null ? '' : String(call.talkgroup));
</script>

<table class="calls">
  <thead>
    <tr>
      <th scope="col"><span class="visually-hidden">Play</span></th>
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
        aria-current={call.id === playingId ? 'true' : undefined}
        data-id={call.id}
      >
        <td class="c-play">
          <button type="button" class="play" onclick={() => onplay(call)}
            aria-label={`Play ${talkgroup(call)} at ${callTime(call.started_at, now)}${isPlayed ? ' (played)' : ''}`}>{isPlayed ? '✓' : '▶'}</button>
        </td>
        {#each shown as column (column.id)}
          {#if column.id === 'time'}
            <td class="c-time time">{callTime(call.started_at, now)}</td>
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
          <RowActions label={`${talkgroup(call)} at ${callTime(call.started_at, now)}`}
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
  .play { background: none; border: 1px solid var(--line); border-radius: 50%; width: 2rem; height: 2rem; cursor: pointer; color: var(--accent); }
  .time, .len { white-space: nowrap; font-variant-numeric: tabular-nums; }
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
     hidden one leaves its place empty. */
  @media (max-width: 760px) {
    thead { position: absolute; width: 1px; height: 1px; overflow: hidden; clip: rect(0 0 0 0); }
    tbody, tr, td { display: block; }
    tr { display: grid; grid-template-columns: auto auto 1fr auto auto; align-items: center; border-bottom: 1px solid var(--line); }
    td { border: 0; padding: 0.2rem 0.4rem; }
    td.extra, th.extra { display: none; }
    .c-play { grid-column: 1; }
    .c-time { grid-column: 2; }
    .c-talkgroup { grid-column: 3; overflow: hidden; text-overflow: ellipsis; }
    .c-length { grid-column: 4; }
    .c-actions { grid-column: 5; }
    .bar { display: none; }
    td.text { grid-column: 1 / -1; padding-top: 0; }
    td.chips { display: flex; flex-wrap: wrap; gap: 0.1rem 0.9rem; grid-column: 1 / -1; padding-top: 0; font-size: 0.85rem; color: var(--muted); overflow-wrap: anywhere; }
    .chip-name { font-weight: 600; }
  }
</style>
