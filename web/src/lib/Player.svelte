<script lang="ts">
  import { view } from './calls.svelte';
  import { callTime, source } from './format';
  import { afterEnd, afterFailure, navKey, step } from './playback';
  import { failed, play, player, stepTo } from './playing.svelte';

  let { continuous = $bindable(), autoplayNew = $bindable() }: { continuous: boolean; autoplayNew: boolean } =
    $props();

  let audio: HTMLAudioElement;
  const id = $derived(player.call?.id ?? null);
  const onPage = $derived(id !== null && view.calls.some((c) => c.id === id));
  const canNewer = $derived(view.calls.length > 0 && (!onPage || step(view.calls, id, 'newer') !== null));
  const canOlder = $derived(view.calls.length > 0 && (!onPage || step(view.calls, id, 'older') !== null));

  $effect(() => {
    if (id === null) return;
    audio.src = `/audio/${id}`;
    // A browser may refuse to start sound before the listener has clicked:
    // the call then waits, paused, for the play button.
    audio.play().catch(() => {});
  });

  function ended() {
    player.busy = false;
    if (id === null) return;
    const next = afterEnd(view.calls, id, continuous, failed);
    if (next) play(next, 'newer');
  }

  function failedToPlay() {
    player.busy = false;
    if (id === null) return;
    failed.add(id);
    const next = afterFailure(view.calls, id, player.direction, failed);
    if (next) play(next, player.direction);
  }

  function key(event: KeyboardEvent) {
    const direction = navKey(event);
    if (!direction) return;
    event.preventDefault();
    stepTo(direction);
  }
</script>

<svelte:window onkeydown={key} />

<div class="player">
  <div class="nav">
    <button type="button" onclick={() => stepTo('newer')} disabled={!canNewer} aria-label="Newer call (N)" title="Newer call (N)">▲</button>
    <button type="button" onclick={() => stepTo('older')} disabled={!canOlder} aria-label="Older call (P)" title="Older call (P)">▼</button>
  </div>
  <audio bind:this={audio} controls preload="none" onended={ended} onerror={failedToPlay}
    onplaying={() => (player.busy = true)} onpause={() => (player.busy = false)}></audio>
  <p class="now" aria-live="polite">
    {#if player.call}
      <b>{player.call.talkgroup_name ?? player.call.talkgroup}</b>
      {callTime(player.call.started_at, new Date())}
      {source(player.call)}
      {#if failed.has(player.call.id)}<span class="failed">could not be played</span>{/if}
    {:else}
      Nothing playing
    {/if}
  </p>
  <label><input type="checkbox" bind:checked={continuous} /> Continuous</label>
  <label><input type="checkbox" bind:checked={autoplayNew} /> Play new calls</label>
</div>

<style>
  .player { display: flex; flex-wrap: wrap; gap: 0.5rem 1rem; align-items: center; padding: 0.5rem 1rem; }
  .nav { display: flex; gap: 0.25rem; }
  .nav button {
    width: 2.5rem; height: 2.25rem; border: 1px solid currentColor; border-radius: 4px;
    background: none; color: var(--bar-text); cursor: pointer;
  }
  .nav button:disabled { opacity: 0.4; cursor: default; }
  audio { height: 2.25rem; flex: 1 1 16rem; min-width: 12rem; }
  .now { margin: 0; flex: 1 1 12rem; }
  .now b { color: var(--accent-strong); margin-right: 0.4rem; }
  .failed { margin-left: 0.4rem; background: var(--warn-bg); color: var(--warn-text); padding: 0 0.3rem; border-radius: 3px; }
  label { display: flex; gap: 0.35rem; align-items: center; white-space: nowrap; }
</style>
