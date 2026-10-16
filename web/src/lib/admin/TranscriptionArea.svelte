<script lang="ts">
  import { onMount } from 'svelte';
  import { tell } from '../notice.svelte';
  import { admin, why } from './session.svelte';
  import { matchTalkgroups, previewText, queueBody, queueQuery, type AdminTalkgroup, type QueueFields } from './transcription';

  let talkgroups = $state<AdminTalkgroup[] | null>(null);
  let problem = $state<string | null>(null);
  let search = $state('');
  const shown = $derived(talkgroups ? matchTalkgroups(talkgroups, search) : []);
  const on = $derived(talkgroups ? talkgroups.filter((t) => t.transcribe).length : 0);
  /** A refusal beside the switch that asked for it. */
  let refusals = $state<Record<number, string>>({});

  async function read() {
    try {
      talkgroups = await admin<AdminTalkgroup[]>('GET', '/api/admin/talkgroups');
      problem = null;
    } catch (error) {
      problem = why(error);
    }
  }
  onMount(read);

  async function setSwitch(talkgroup: AdminTalkgroup, value: boolean) {
    delete refusals[talkgroup.id];
    try {
      await admin('PUT', `/api/admin/talkgroups/${talkgroup.id}/transcribe`, { on: value });
      talkgroup.transcribe = value;
    } catch (error) {
      refusals[talkgroup.id] = why(error);
    }
  }

  // The queue job: counted first, then asked for.
  const fields = $state<QueueFields>({ talkgroup: null, from: '', to: '' });
  let preview = $state<{ count: number; min_ms: number } | null>(null);
  let queueRefusal = $state<string | null>(null);
  let busy = $state(false);
  // A changed field makes the count stale: it must be counted again.
  $effect(() => {
    void [fields.talkgroup, fields.from, fields.to];
    preview = null;
  });

  async function count(event: SubmitEvent) {
    event.preventDefault();
    queueRefusal = null;
    busy = true;
    try {
      preview = await admin<{ count: number; min_ms: number }>('GET', `/api/admin/jobs/queue?${queueQuery(fields)}`);
    } catch (error) {
      queueRefusal = why(error);
    } finally {
      busy = false;
    }
  }

  async function queue() {
    queueRefusal = null;
    busy = true;
    try {
      const job = await admin<{ id: number; count: number }>('POST', '/api/admin/jobs/queue', queueBody(fields));
      tell(`Job #${job.id} queues ${job.count.toLocaleString('en-US')} calls; follow it in Jobs.`);
      preview = null;
    } catch (error) {
      queueRefusal = why(error);
    } finally {
      busy = false;
    }
  }

  let retried = $state<string | null>(null);
  async function retry() {
    retried = null;
    try {
      const answer = await admin<{ retried: number }>('POST', '/api/admin/queue/retry');
      retried = answer.retried === 0 ? 'No failed call to try again.' : `${answer.retried.toLocaleString('en-US')} failed calls are back in the queue.`;
    } catch (error) {
      retried = `The failed calls were not put back: ${why(error)}`;
    }
  }
</script>

<section class="area" aria-labelledby="transcription-title">
  <h1 id="transcription-title">Transcription</h1>
  <p class="lead">
    New calls of a talkgroup are transcribed while its switch is on. Past calls are queued by a job, counted
    first. Transcription itself runs only while the setting <code>transcription.enabled</code> is true.
  </p>
  {#if problem}<p class="problem" role="alert">The talkgroups could not be read: {problem}</p>{/if}

  <h2>Talkgroups</h2>
  {#if talkgroups === null}
    <p class="empty">Reading…</p>
  {:else}
    <div class="row">
      <label for="tg-search">Find a talkgroup</label>
      <input id="tg-search" type="search" bind:value={search} />
      <span>{on.toLocaleString('en-US')} of {talkgroups.length.toLocaleString('en-US')} on</span>
    </div>
    <table>
      <caption class="visually-hidden">Each talkgroup's transcription switch</caption>
      <thead><tr><th scope="col">System</th><th scope="col" class="num">Number</th><th scope="col">Name</th><th scope="col">Transcribe</th></tr></thead>
      <tbody>
        {#each shown as talkgroup (talkgroup.id)}
          <tr>
            <td>{talkgroup.system}</td>
            <td class="num">{talkgroup.number}</td>
            <td class="long">{talkgroup.name}</td>
            <td>
              <input type="checkbox" aria-label={`Transcribe ${talkgroup.name} (${talkgroup.system} ${talkgroup.number})`}
                checked={talkgroup.transcribe} onchange={(event) => setSwitch(talkgroup, event.currentTarget.checked)} />
              {#if refusals[talkgroup.id]}<p class="problem" role="alert">Not changed: {refusals[talkgroup.id]}</p>{/if}
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}

  <h2>Queue past calls</h2>
  <form class="queue" onsubmit={count}>
    <label for="queue-talkgroup">Talkgroup</label>
    <select id="queue-talkgroup" bind:value={fields.talkgroup}>
      <option value={null}>Every talkgroup whose transcription is on (the backlog)</option>
      {#each talkgroups ?? [] as talkgroup (talkgroup.id)}
        <option value={talkgroup.id}>{talkgroup.name} · {talkgroup.system} {talkgroup.number}</option>
      {/each}
    </select>
    <div class="row">
      <label>From <input type="date" bind:value={fields.from} /></label>
      <label>To <input type="date" bind:value={fields.to} /></label>
    </div>
    <p class="hint">Whole days in the site's zone; with no day, every past call not yet tried.</p>
    <div class="row">
      <button type="submit" disabled={busy}>Count the calls</button>
      {#if preview && preview.count > 0}
        <button type="button" class="primary" disabled={busy} onclick={queue}>Queue {preview.count.toLocaleString('en-US')} calls</button>
      {/if}
    </div>
    {#if preview}<p role="status">{previewText(preview.count, preview.min_ms)}</p>{/if}
    {#if queueRefusal}<p class="problem" role="alert">Not queued: {queueRefusal}</p>{/if}
  </form>

  <h2>Failed calls</h2>
  <p>A call fails after its tries run out. This puts every failed call back in the queue; a call of a talkgroup whose transcription is off stays failed.</p>
  <button type="button" onclick={retry}>Try the failed calls again</button>
  {#if retried}<p role="status">{retried}</p>{/if}
</section>

<style>
  .queue { display: grid; gap: 0.4rem; max-width: 36rem; }
  .queue label { font-weight: 600; }
  .hint { margin: 0; color: var(--muted); }
  select { max-width: 100%; }
</style>
