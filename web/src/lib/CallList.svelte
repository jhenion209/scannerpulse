<script lang="ts">
  import { SvelteSet } from 'svelte/reactivity';
  import type { Call } from './api';
  import { callTime, lengthPercent, seconds, source } from './format';

  let {
    calls,
    playingId,
    played,
    showAll,
    onplay,
  }: {
    calls: Call[];
    playingId: number | null;
    played: Set<number>;
    showAll: boolean;
    onplay: (call: Call) => void;
  } = $props();

  const expanded = new SvelteSet<number>();
  // Read again with each page, so "today" moves on with the list.
  const now = $derived.by(() => {
    void calls;
    return new Date();
  });
  const talkgroup = (call: Call) => call.talkgroup_name ?? (call.talkgroup === null ? '' : String(call.talkgroup));
</script>

<table class="calls">
  <thead>
    <tr>
      <th scope="col"><span class="visually-hidden">Play</span></th>
      <th scope="col">Time</th>
      <th scope="col">Talkgroup</th>
      <th scope="col" class="wide">Site</th>
      <th scope="col" class="wide">Source</th>
      <th scope="col">Length</th>
      <th scope="col" class="text">Text</th>
    </tr>
  </thead>
  <tbody>
    {#each calls as call (call.id)}
      {@const open = showAll || expanded.has(call.id)}
      {@const isPlayed = played.has(call.id) && call.id !== playingId}
      <tr
        class:playing={call.id === playingId}
        class:played={isPlayed}
        aria-current={call.id === playingId ? 'true' : undefined}
        data-id={call.id}
      >
        <td>
          <button type="button" class="play" onclick={() => onplay(call)}
            aria-label={`Play ${talkgroup(call)} at ${callTime(call.started_at, now)}${isPlayed ? ' (played)' : ''}`}>{isPlayed ? '✓' : '▶'}</button>
        </td>
        <td class="time">{callTime(call.started_at, now)}</td>
        <td class="tg">{talkgroup(call)}</td>
        <td class="wide">{call.site ?? ''}</td>
        <td class="wide">{source(call)}</td>
        <td class="len">
          <span class="bar" aria-hidden="true"><i style:width={`${lengthPercent(call.duration_ms)}%`}></i></span>
          <span class="secs">{seconds(call.duration_ms)}</span>
        </td>
        <td class="text"><div class="line">
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
  .tg, td.wide { white-space: nowrap; }
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
  /* A phone shows each call on two lines: play, time, talkgroup and length,
     then the text across the whole width. */
  @media (max-width: 760px) {
    thead { position: absolute; width: 1px; height: 1px; overflow: hidden; clip: rect(0 0 0 0); }
    tbody, tr, td { display: block; }
    tr { display: grid; grid-template-columns: auto auto 1fr auto; align-items: center; border-bottom: 1px solid var(--line); }
    td { border: 0; padding: 0.2rem 0.4rem; }
    .wide { display: none; }
    .tg { overflow: hidden; text-overflow: ellipsis; }
    .bar { display: none; }
    td.text { grid-column: 1 / -1; padding-top: 0; }
  }
</style>
