<script lang="ts">
  import type { CallDetail } from './api';
  import { seconds, source } from './format';

  let { call, error, onplay, onclose }: {
    call: CallDetail | null;
    error: string | null;
    onplay: (call: CallDetail) => void;
    onclose: () => void;
  } = $props();

  const when = (startedAt: string) =>
    new Date(startedAt).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'medium' });
</script>

<section class="shared" aria-labelledby="shared-title">
  <h2 id="shared-title">Shared call</h2>
  {#if call}
    <div class="row">
      <button type="button" class="play" onclick={() => onplay(call)}
        aria-label={`Play ${call.talkgroup_name ?? call.talkgroup} at ${when(call.started_at)}`}>▶</button>
      <div class="facts">
        <b>{call.talkgroup_name ?? call.talkgroup}</b>
        <span>{when(call.started_at)}</span>
        <span>{seconds(call.duration_ms)}</span>
        {#if source(call)}<span>{source(call)}</span>{/if}
      </div>
    </div>
    <p class="text">{call.transcript ?? 'No text'}</p>
  {:else if error}
    <p class="text">{error}</p>
  {:else}
    <p class="text">Reading the call…</p>
  {/if}
  <button type="button" class="close" onclick={onclose}>Close</button>
</section>

<style>
  .shared {
    position: relative; margin: 0.75rem; padding: 0.75rem 0.9rem; border: 1px solid var(--field-border);
    border-left: 4px solid var(--accent); border-radius: 6px; background: var(--surface);
  }
  h2 { margin: 0 0 0.4rem; font-size: 0.85rem; text-transform: uppercase; color: var(--muted); }
  .row { display: flex; gap: 0.7rem; align-items: center; }
  .play {
    flex: none; width: 2.25rem; height: 2.25rem; border-radius: 50%; border: 1px solid var(--field-border);
    background: var(--bg); color: var(--accent); cursor: pointer;
  }
  .facts { display: flex; flex-wrap: wrap; gap: 0.2rem 0.8rem; align-items: baseline; }
  .text { margin: 0.5rem 0 0; white-space: pre-wrap; overflow-wrap: anywhere; }
  .close {
    position: absolute; top: 0.5rem; right: 0.6rem; min-height: 2rem; padding: 0 0.6rem;
    border: 1px solid var(--field-border); border-radius: 4px; background: var(--bg); cursor: pointer;
  }
</style>
