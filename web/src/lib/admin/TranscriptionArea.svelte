<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { counted } from '../format';
  import { tell } from '../notice.svelte';
  import ImprovementSection from './ImprovementSection.svelte';
  import { admin, can, why } from './session.svelte';
  import {
    matchTalkgroups, nextSort, previewText, queueBody, queueQuery, retriedText, sortTalkgroups,
    type AdminTalkgroup, type QueueFields, type TalkgroupColumn, type TalkgroupSort,
  } from './transcription';

  let talkgroups = $state<AdminTalkgroup[] | null>(null);
  let problem = $state<string | null>(null);
  let search = $state('');
  /** System, then number (the server's order) until a header is clicked. */
  let sort = $state<TalkgroupSort>({ column: 'system', descending: false });
  const shown = $derived(talkgroups ? sortTalkgroups(matchTalkgroups(talkgroups, search), sort) : []);
  const columns: { key: TalkgroupColumn; label: string; num?: boolean }[] = [
    { key: 'system', label: 'System' }, { key: 'number', label: 'Number', num: true },
    { key: 'name', label: 'Name' }, { key: 'transcribe', label: 'Transcribe' },
  ];
  const ariaSort = (key: TalkgroupColumn) => (sort.column !== key ? 'none' : sort.descending ? 'descending' : 'ascending');
  const on = $derived(talkgroups ? talkgroups.filter((t) => t.transcribe).length : 0);
  /** A refusal beside the switch that asked for it. */
  let refusals = $state<Record<number, string>>({});

  /** Another admin may have changed the talkgroups since they were read: after a refusal or a start they are read again. */
  async function read() {
    try {
      talkgroups = await admin<AdminTalkgroup[]>('GET', '/api/admin/talkgroups');
      problem = null;
    } catch (error) {
      problem = why(error);
    }
  }
  onMount(read);

  async function setSwitch(talkgroup: AdminTalkgroup, box: HTMLInputElement) {
    const value = box.checked;
    const focused = document.activeElement === box;
    delete refusals[talkgroup.id];
    try {
      await admin('PUT', `/api/admin/talkgroups/${talkgroup.id}/transcribe`, { on: value });
      talkgroup.transcribe = value;
      // Sorted by Transcribe, the row moves, and a moved element loses the focus: give it back.
      if (focused) {
        await tick();
        box.focus();
      }
    } catch (error) {
      refusals[talkgroup.id] = why(error);
      // `checked` is one-way: nothing writes the saved state back to a box the person toggled.
      box.checked = talkgroup.transcribe;
      await read();
    }
  }

  // The queue job: counted first, then asked for.
  const fields = $state<QueueFields>({ talkgroup: null, from: '', to: '' });
  // The count, with the request it was counted for: that request is what gets queued.
  let preview = $state<{ count: number; min_ms: number; body: ReturnType<typeof queueBody> } | null>(null);
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
    const body = queueBody(fields);
    // An answer for fields that have changed since is dropped.
    const current = () => JSON.stringify(body) === JSON.stringify(queueBody(fields));
    try {
      const found = await admin<{ count: number; min_ms: number }>('GET', `/api/admin/jobs/queue?${queueQuery(fields)}`);
      if (current()) preview = { ...found, body };
    } catch (error) {
      if (current()) {
        queueRefusal = why(error);
        await read();
      }
    } finally {
      busy = false;
    }
  }

  async function queue() {
    if (!preview) return;
    queueRefusal = null;
    busy = true;
    try {
      const job = await admin<{ id: number; count: number }>('POST', '/api/admin/jobs/queue', preview.body);
      tell(`Job #${job.id} queues ${counted(job.count, 'call')}; follow it in Jobs.`);
      preview = null;
      await read();
    } catch (error) {
      queueRefusal = why(error);
      await read();
    } finally {
      busy = false;
    }
  }

  let retried = $state<string | null>(null);
  let retryRefusal = $state<string | null>(null);
  let retrying = $state(false);
  async function retry() {
    if (retrying) return;
    retried = retryRefusal = null;
    retrying = true;
    try {
      const answer = await admin<{ retried: number }>('POST', '/api/admin/queue/retry');
      retried = retriedText(answer.retried);
    } catch (error) {
      retryRefusal = why(error);
    } finally {
      retrying = false;
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
      <thead><tr>
        {#each columns as column (column.key)}
          <th scope="col" class:num={column.num} aria-sort={ariaSort(column.key)}>
            <button type="button" class="sort" onclick={() => (sort = nextSort(sort, column.key))}>{column.label}<span aria-hidden="true">{ariaSort(column.key) === 'ascending' ? ' ▲' : ariaSort(column.key) === 'descending' ? ' ▼' : ''}</span></button>
          </th>
        {/each}
      </tr></thead>
      <tbody>
        {#each shown as talkgroup (talkgroup.id)}
          <tr>
            <td>{talkgroup.system}</td>
            <td class="num">{talkgroup.number}</td>
            <td class="long">{talkgroup.name}</td>
            <td>
              <input type="checkbox" aria-label={`Transcribe ${talkgroup.name} (${talkgroup.system} ${talkgroup.number})`}
                checked={talkgroup.transcribe} disabled={!can('transcribe')} onchange={(event) => setSwitch(talkgroup, event.currentTarget)} />
              {#if refusals[talkgroup.id]}<p class="problem" role="alert">Not changed: {refusals[talkgroup.id]}</p>{/if}
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}

  {#if can('queue')}
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
        <button type="button" class="primary" disabled={busy} onclick={queue}>Queue {counted(preview.count, 'call')}</button>
      {/if}
    </div>
    {#if preview}<p role="status">{previewText(preview.count, preview.min_ms)}</p>{/if}
    {#if queueRefusal}<p class="problem" role="alert">Not queued: {queueRefusal}</p>{/if}
  </form>

  <h2>Failed calls</h2>
  <p>A call fails after its tries run out. This puts every failed call back in the queue; a call of a talkgroup whose transcription is off stays failed.</p>
  <button type="button" disabled={retrying} onclick={retry}>Try the failed calls again</button>
  {#if retried}<p role="status">{retried}</p>{/if}
  {#if retryRefusal}<p class="problem" role="alert">The failed calls were not put back: {retryRefusal}</p>{/if}
  {/if}

  <ImprovementSection />
</section>

<style>
  /* One column that may shrink below its widest content (a select's longest option),
     else the track grows to it and the form outgrows a phone's width. */
  .queue { display: grid; grid-template-columns: minmax(0, 1fr); gap: 0.4rem; max-width: 36rem; }
  .queue label { font-weight: 600; }
  .hint { margin: 0; color: var(--muted); }
  select { max-width: 100%; }
  .sort { background: none; border: 0; padding: 0; font: inherit; color: inherit; cursor: pointer; text-align: inherit; }
</style>
