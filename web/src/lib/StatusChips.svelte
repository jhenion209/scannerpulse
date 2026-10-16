<script lang="ts">
  import { ageOf, callsText, clockText, durationText, etaText, isStale } from './status';
  import { serverNow, status } from './status.svelte';

  const age = $derived(status.data ? ageOf(status.data.newest_call, serverNow()) : null);
  const stale = $derived(isStale(age));
</script>

{#if status.data}
  <ul class="chips" aria-label="Status">
    {#if status.data.maintenance}<li class="chip warn">Maintenance</li>{/if}
    <li class="chip wide">{callsText(status.data.calls, status.data.calls_estimated)}</li>
    {#if status.data.transcription}
      <li class="chip wide">
        Queue {status.data.queue.toLocaleString('en-US')}{status.data.queue && status.data.eta_seconds !== null ? ` · ${etaText(status.data.eta_seconds)}` : ''}
      </li>
    {/if}
    <li class="chip" class:warn={stale} class:wide={!stale}>
      {age === null ? 'No calls yet' : `Newest call ${durationText(age)} ago`}
    </li>
    <li class="chip wide"><time>{clockText(serverNow(), status.data.time_zone)}</time></li>
  </ul>
{:else if status.error}
  <ul class="chips" aria-label="Status"><li class="chip wide">Status unavailable</li></ul>
{/if}

<style>
  .chips { display: flex; flex-wrap: wrap; gap: 0.4rem; margin: 0; padding: 0; list-style: none; }
  .chip {
    font-size: 0.85rem; line-height: 1.3; padding: 0.15rem 0.55rem; border-radius: 999px;
    border: 1px solid color-mix(in srgb, var(--bar-text) 45%, transparent); color: var(--bar-text);
    white-space: nowrap; font-variant-numeric: tabular-nums;
  }
  .warn { background: var(--warn-bg); color: var(--warn-text); border-color: var(--warn-text); font-weight: 600; }
  /* A phone keeps only what needs attention, maintenance and a stale feed,
     on a row of its own under the bar; with nothing to show, no row. */
  @media (max-width: 760px) {
    .wide { display: none; }
    .chips { flex-basis: 100%; order: 10; }
    .chips:not(:has(.chip:not(.wide))) { display: none; }
    .chip { white-space: normal; }
  }
</style>
