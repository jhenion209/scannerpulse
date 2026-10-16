<script lang="ts">
  import { onMount } from 'svelte';
  import { fetchStatus, type Status } from '../api';
  import { tell } from '../notice.svelte';
  import { etaText } from '../status';
  import { drained } from './drain';
  import { STATUS_EVERY_MS } from './health';
  import { admin, why } from './session.svelte';

  let data = $state<Status | null>(null);
  let problem = $state<string | null>(null);
  let refusal = $state<string | null>(null);
  let busy = $state(false);
  // The largest the queue was seen at since the area opened: the bar's 0 %.
  let largest = $state(0);
  const share = $derived(data ? drained(largest, data.queue) : null);

  async function read() {
    try {
      data = await fetchStatus();
      largest = Math.max(largest, data.queue);
      problem = null;
    } catch (error) {
      problem = why(error);
    }
  }

  onMount(() => {
    void read();
    const timer = setInterval(read, STATUS_EVERY_MS);
    return () => clearInterval(timer);
  });

  /** Turns maintenance on or off: the same setting the command line sets. */
  async function setMaintenance(on: boolean) {
    busy = true;
    refusal = null;
    try {
      await admin('PUT', '/api/admin/settings/maintenance.enabled', { value: on });
      await read();
    } catch (error) {
      refusal = why(error);
    } finally {
      busy = false;
    }
  }

  let rescanRefusal = $state<string | null>(null);
  async function rescan() {
    rescanRefusal = null;
    try {
      await admin('POST', '/api/admin/rescan');
      tell('Every running ingest lists its sources now.');
    } catch (error) {
      rescanRefusal = why(error);
    }
  }
</script>

<section class="area" aria-labelledby="maintenance-title">
  <h1 id="maintenance-title">Maintenance</h1>
  <p class="lead">
    While maintenance is on, ingest files nothing, transcribes nothing and runs no job, on every host, and
    keeps the lease; saving a text answers that the site is under maintenance. The archive or a source can
    then be serviced.
  </p>
  {#if problem}<p class="problem" role="alert">The status could not be read: {problem}</p>{/if}

  <h2>Maintenance</h2>
  {#if data}
    <p>Maintenance is <b>{data.maintenance ? 'on' : 'off'}</b>.</p>
    <button type="button" class="primary" disabled={busy} onclick={() => setMaintenance(!data!.maintenance)}>
      {data.maintenance ? 'Turn maintenance off' : 'Turn maintenance on'}
    </button>
    {#if refusal}<p class="problem" role="alert">Maintenance was not changed: {refusal}</p>{/if}
  {:else}
    <p class="empty">Reading…</p>
  {/if}

  <h2>Rescan the sources</h2>
  <p>Ingest lists its sources every few seconds; this asks every running ingest to list them at once.</p>
  <button type="button" onclick={rescan}>Rescan now</button>
  {#if rescanRefusal}<p class="problem" role="alert">The rescan was not asked for: {rescanRefusal}</p>{/if}

  <h2>Transcription queue</h2>
  {#if data}
    <p>
      {data.queue.toLocaleString('en-US')} {data.queue === 1 ? 'call' : 'calls'} waiting{#if data.queue && data.eta_seconds !== null},
        done in {etaText(data.eta_seconds)}{/if}{#if !data.transcription}; transcription is off{/if}.
    </p>
    {#if share !== null}
      <label for="drain">Drained since {largest.toLocaleString('en-US')} were waiting</label>
      <progress id="drain" max="100" value={Math.round(share * 100)}>{Math.round(share * 100)} %</progress>
      <span>{Math.round(share * 100)} %</span>
    {/if}
  {/if}
</section>

<style>
  label { display: block; margin: 0.6rem 0 0.2rem; }
  progress { width: min(24rem, 70%); height: 1rem; vertical-align: middle; accent-color: var(--accent); }
</style>
