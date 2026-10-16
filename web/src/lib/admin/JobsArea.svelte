<script lang="ts">
  import { callTime } from '../format';
  import { siteZone } from '../status.svelte';
  import { canStop, kindName, percent, progressText, stateText, waitingFor, type Job } from './jobs';
  import { jobs, readJobs } from './jobs.svelte';
  import { admin, why } from './session.svelte';

  // A refusal or an answer beside the job whose Stop asked for it.
  let answers = $state<Record<number, { text: string; refused: boolean }>>({});
  let stopping = $state<number | null>(null);

  async function stop(job: Job) {
    stopping = job.id;
    try {
      const answer = await admin<{ stopped: string }>('POST', `/api/admin/jobs/${job.id}/stop`);
      answers[job.id] = { text: `Stopped ${answer.stopped}.`, refused: false };
      await readJobs();
    } catch (error) {
      answers[job.id] = { text: `The job was not stopped: ${why(error)}`, refused: true };
    } finally {
      stopping = null;
    }
  }

  const when = (at: string | null) => (at === null ? '—' : callTime(at, new Date(), siteZone()));
</script>

<section class="area" aria-labelledby="jobs-title">
  <h1 id="jobs-title">Jobs</h1>
  <div class="row lead">
    <span>The long jobs, newest first. The host that holds the lease runs one at a time, a step at a time; a stop takes effect between steps. Read every 10 s.</span>
    <button type="button" onclick={readJobs}>Read now</button>
  </div>
  {#if jobs.error}<p class="problem" role="alert">The jobs could not be read: {jobs.error}</p>{/if}
  {#if jobs.list === null}
    <p class="empty">Reading…</p>
  {:else if jobs.list.length === 0}
    <p class="empty">No job has run yet.</p>
  {:else}
    <ol class="jobs">
      {#each jobs.list as job (job.id)}
        {@const share = percent(job)}
        {@const reason = waitingFor(job)}
        <li class="job" aria-labelledby={`job-${job.id}`}>
          <h2 id={`job-${job.id}`}>{kindName(job.kind)} <span class="id">#{job.id}</span></h2>
          <p class="state">
            <span class={job.state === 'failed' ? 'flag' : 'tag'}>{stateText(job)}</span>
            {progressText(job)}
            {#if share !== null}<progress max="100" value={share} aria-label={`Job ${job.id} done`}>{share} %</progress> {share} %{/if}
          </p>
          {#if reason}
            <p><span class="flag">Waiting</span> {reason}</p>
          {:else if job.message}
            <p class="message">{job.message}</p>
          {/if}
          <dl class="facts">
            <dt>Started by</dt><dd>{job.started_by}</dd>
            <dt>Asked for</dt><dd>{when(job.created_at)}</dd>
            {#if job.started_at}<dt>Started</dt><dd>{when(job.started_at)}{job.host ? ` on ${job.host}` : ''}</dd>{/if}
            {#if job.finished_at}<dt>Ended</dt><dd>{when(job.finished_at)}</dd>
            {:else if job.updated_at}<dt>Last step</dt><dd>{when(job.updated_at)}</dd>{/if}
          </dl>
          {#if canStop(job)}
            <button type="button" disabled={stopping === job.id} onclick={() => stop(job)}>Stop job #{job.id}</button>
          {/if}
          {#if answers[job.id]}
            <p class={answers[job.id].refused ? 'problem' : 'said'} role={answers[job.id].refused ? 'alert' : 'status'}>{answers[job.id].text}</p>
          {/if}
        </li>
      {/each}
    </ol>
  {/if}
</section>

<style>
  .jobs { list-style: none; margin: 0; padding: 0; display: grid; gap: 0.75rem; }
  .job { border: 1px solid var(--line); border-radius: 6px; padding: 0.7rem 0.9rem; background: var(--surface); }
  .job h2 { margin: 0 0 0.3rem; font-size: 1.05rem; }
  .id { color: var(--muted); font-weight: 400; }
  .state { display: flex; flex-wrap: wrap; gap: 0.5rem; align-items: center; margin: 0.2rem 0; }
  .tag { padding: 0 0.4rem; border-radius: 3px; border: 1px solid var(--field-border); }
  progress { width: min(14rem, 50%); accent-color: var(--accent); }
  .message { margin: 0.3rem 0; overflow-wrap: anywhere; }
  .facts { margin: 0.4rem 0; }
  .said { margin: 0.4rem 0 0; }
</style>
