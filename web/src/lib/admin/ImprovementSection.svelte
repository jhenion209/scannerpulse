<script lang="ts">
  import { onMount } from 'svelte';
  import { ApiError } from '../api';
  import { callTime } from '../format';
  import { tell } from '../notice.svelte';
  import { siteZone } from '../status.svelte';
  import { admin, why } from './session.svelte';
  import { werText, type AsrReport, type AsrSet } from './improvement';

  let sets = $state<AsrSet[] | null>(null);
  /** The server's words while the module is off. */
  let off = $state<string | null>(null);
  let problem = $state<string | null>(null);
  let report = $state<AsrReport | null>(null);

  onMount(async () => {
    try {
      sets = await admin<AsrSet[]>('GET', '/api/admin/asr/sets');
    } catch (error) {
      if (error instanceof ApiError && error.status === 403) off = why(error);
      else problem = why(error);
    }
  });

  async function show(set: string) {
    problem = null;
    try {
      report = await admin<AsrReport>('GET', `/api/admin/asr/report?${new URLSearchParams({ set })}`);
    } catch (error) {
      problem = why(error);
    }
  }

  const candidate = $state({ server: '', model: '', prompt: '' });
  let refusal = $state<string | null>(null);
  let busy = $state(false);
  async function run(event: SubmitEvent) {
    event.preventDefault();
    if (!report) return;
    const set = report.set;
    refusal = null;
    busy = true;
    try {
      const job = await admin<{ id: number; count: number }>('POST', '/api/admin/jobs/asr-run', {
        set,
        server: candidate.server.trim(),
        model: candidate.model,
        prompt: candidate.prompt.trim() || undefined,
      });
      tell(`Job #${job.id} runs ${job.count.toLocaleString('en-US')} calls of the set ${set}; follow it in Jobs.`);
    } catch (error) {
      refusal = why(error);
    } finally {
      busy = false;
    }
  }
</script>

<h2>Improvement</h2>
<p>
  The improvement module scores transcription against people's texts of sample calls, and runs a candidate (a
  server, a model, a prompt) on the same calls to compare.
</p>
{#if off}
  <p class="empty">Not shown: {off}.</p>
{:else if problem}
  <p class="problem" role="alert">{problem}</p>
{/if}
{#if sets}
  {#if sets.length === 0}
    <p class="empty">No set yet; <code>scannerpulse asr sample</code> makes one.</p>
  {:else}
    <table>
      <caption class="visually-hidden">The improvement module's sets</caption>
      <thead><tr><th scope="col">Set</th><th scope="col" class="num">Calls</th><th scope="col" class="num">With a person's text</th><th scope="col" class="num">Runs</th><th scope="col"><span class="visually-hidden">Report</span></th></tr></thead>
      <tbody>
        {#each sets as set (set.set)}
          <tr>
            <td class="long">{set.set}</td>
            <td class="num">{set.size.toLocaleString('en-US')}</td>
            <td class="num">{set.labeled.toLocaleString('en-US')}</td>
            <td class="num">{set.runs.toLocaleString('en-US')}</td>
            <td><button type="button" aria-label={`Show the report of ${set.set}`} onclick={() => show(set.set)}>Report</button></td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
{/if}

{#if report}
  <h3>The set {report.set}</h3>
  <p>
    {report.size.toLocaleString('en-US')} calls, {report.labeled.toLocaleString('en-US')} with a person's text.<br />
    Production on its calls: {report.production.summary}
  </p>
  {#if report.runs.length === 0}
    <p class="empty">No run yet.</p>
  {:else}
    <table>
      <caption class="visually-hidden">Runs on the set {report.set}</caption>
      <thead><tr><th scope="col">When</th><th scope="col">Candidate</th><th scope="col" class="num">Calls</th><th scope="col" class="num">WER</th><th scope="col" class="num">Seconds a call</th></tr></thead>
      <tbody>
        {#each report.runs as run, index (index)}
          <tr>
            <td>{callTime(run.run_at, new Date(), siteZone())}</td>
            <td class="long">
              {run.model} at {run.server}
              {#if run.prompt}<br /><span class="muted">Prompt: {run.prompt}</span>{/if}
            </td>
            <td class="num">{run.calls.toLocaleString('en-US')}</td>
            <td class="num">{werText(run.wer)}</td>
            <td class="num">{run.seconds_per_call.toFixed(2)}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}

  <h3>Run a candidate on {report.set}</h3>
  <form class="candidate" onsubmit={run}>
    <label for="candidate-server">Server</label>
    <input id="candidate-server" type="text" inputmode="url" autocomplete="off" bind:value={candidate.server} />
    <label for="candidate-model">Model</label>
    <input id="candidate-model" type="text" autocomplete="off" bind:value={candidate.model} />
    <label for="candidate-prompt">Prompt (optional)</label>
    <textarea id="candidate-prompt" rows="3" bind:value={candidate.prompt}></textarea>
    <p class="hint">
      The set's calls with a person's text go to this server, one a step. A key goes in
      <code>SCANNERPULSE_TRANSCRIPTION_API_KEY</code>, never in the address.
    </p>
    <div class="row"><button type="submit" class="primary" disabled={busy}>Start the run</button></div>
    {#if refusal}<p class="problem" role="alert">Not started: {refusal}</p>{/if}
  </form>
{/if}

<style>
  h3 { margin: 1.2rem 0 0.4rem; font-size: 1rem; }
  .candidate { display: grid; gap: 0.4rem; max-width: 36rem; }
  .candidate label { font-weight: 600; }
  .hint { margin: 0; color: var(--muted); }
  .muted { color: var(--muted); }
</style>
