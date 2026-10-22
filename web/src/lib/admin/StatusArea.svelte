<script lang="ts">
  import { onMount } from 'svelte';
  import { callTime } from '../format';
  import { durationText } from '../status';
  import { siteZone } from '../status.svelte';
  import { about, failoverChecks, leaseText, locationWarnings, refereeWarning, STATUS_EVERY_MS, transcriptState, type AdminStatus } from './health';
  import { admin, why } from './session.svelte';

  let data = $state<AdminStatus | null>(null);
  let readAt = $state<Date | null>(null);
  let problem = $state<string | null>(null);
  let reading = false;

  /** Reads the status; a read still under way is not started again. A
   * failed read keeps the last good one on screen, with the failure above it.
   * The timer's reads are polls: they do not count as use of the session. */
  async function read(poll = false) {
    if (reading) return;
    reading = true;
    try {
      data = await admin<AdminStatus>('GET', '/api/admin/status', undefined, { poll });
      readAt = new Date();
      problem = null;
    } catch (error) {
      problem = why(error);
    } finally {
      reading = false;
    }
  }

  onMount(() => {
    void read();
    const timer = setInterval(() => void read(true), STATUS_EVERY_MS);
    return () => clearInterval(timer);
  });

  const when = (at: string | null) => (at === null ? '—' : callTime(at, new Date(), siteZone()));
  const count = (n: number) => n.toLocaleString('en-US');
  const seconds = (s: number) => durationText(Math.abs(s) * 1000);
</script>

<section class="area" aria-labelledby="status-title">
  <h1 id="status-title">Status</h1>
  <div class="row lead">
    <span>
      {#if readAt}Read at {callTime(readAt.toISOString(), new Date(), siteZone())}; read again every 15 s.{:else}Reading…{/if}
    </span>
    <button type="button" onclick={() => read()}>Read now</button>
  </div>
  {#if problem}<p class="problem" role="alert">The status could not be read: {problem}</p>{/if}

  {#if data}
    {@const warning = refereeWarning(data.referee)}
    {#if warning}<p class="problem" role="alert">{warning.text}{#if warning.reason} <span class="muted">({warning.reason})</span>{/if}</p>{/if}
    {#each locationWarnings(data.locations) as text (text)}<p class="problem" role="alert">{text}</p>{/each}
    <h2>Processing</h2>
    <dl class="facts">
      <dt>Lease</dt>
      <dd>{leaseText(data.lease)}</dd>
      <dt>Maintenance</dt>
      <dd>{#if data.maintenance}<span class="flag">on</span>: filing, transcription and jobs are paused{:else}off{/if}</dd>
    </dl>
    {#if data.lease.heartbeats.length}
      <table>
        <caption class="visually-hidden">Each host's heartbeat</caption>
        <thead><tr><th scope="col">Host</th><th scope="col">Renewed</th><th scope="col">Runs out</th></tr></thead>
        <tbody>
          {#each data.lease.heartbeats as beat (beat.host)}
            <tr>
              <td class="long">{beat.host}</td>
              <td>{seconds(beat.age)} ago</td>
              <td>{beat.expires_in >= 0 ? `in ${seconds(beat.expires_in)}` : `${seconds(beat.expires_in)} ago`}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    {/if}

    <h2>After a failover</h2>
    <ul class="checks">
      {#each failoverChecks(data.failover) as check (check.label)}
        <li><span class={check.ok ? 'ok' : 'flag'}>{check.ok ? '✓' : '!'}</span> {check.label}: <b>{check.value}</b></li>
      {/each}
      <li><span class="ok">·</span> Jobs running: <b>{count(data.failover.jobs_running)}</b></li>
    </ul>

    <h2>Ingest</h2>
    <dl class="facts">
      <dt>Files waiting</dt>
      <dd>{count(data.ingest.waiting)}</dd>
      <dt>Files refused</dt>
      <dd>{count(data.ingest.rejected)}</dd>
    </dl>
    {#if data.ingest.recent_rejected.length}
      <table>
        <caption class="visually-hidden">The files refused most recently</caption>
        <thead><tr><th scope="col">Seen</th><th scope="col">Source</th><th scope="col">File</th><th scope="col">Why</th></tr></thead>
        <tbody>
          {#each data.ingest.recent_rejected as file, i (i)}
            <tr><td>{when(file.seen_at)}</td><td class="long">{file.source}</td><td class="long">{file.rel_path}</td><td class="long">{file.reason ?? ''}</td></tr>
          {/each}
        </tbody>
      </table>
    {/if}

    <h2>Transcription</h2>
    <dl class="facts">
      <dt>Queued</dt>
      <dd>{count(data.queue.queued)} ({count(data.queue.due)} due now)</dd>
      <dt>Queued in the last minute</dt>
      <dd>{count(data.queue.last_minute)}</dd>
      <dt>Failed</dt>
      <dd>{count(data.queue.failed)}</dd>
    </dl>
    {#if data.servers.length}
      <table>
        <caption class="visually-hidden">The transcription servers</caption>
        <thead><tr><th scope="col">Server</th><th scope="col">Resting until</th><th scope="col" class="num">Failed rounds</th><th scope="col">Last error</th></tr></thead>
        <tbody>
          {#each data.servers as server (server.url)}
            <tr>
              <td class="long">{server.url}</td>
              <td>{when(server.cooling_until)}</td>
              <td class="num">{count(server.failed_rounds)}</td>
              <td class="long">{server.last_error ?? ''}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    {:else}
      <p class="empty">No transcription server has answered yet.</p>
    {/if}
    {#if data.failed.length}
      <table>
        <caption class="visually-hidden">The calls whose transcription failed</caption>
        <thead><tr><th scope="col">Call</th><th scope="col">Time</th><th scope="col" class="num">Tries</th><th scope="col">Last error</th></tr></thead>
        <tbody>
          {#each data.failed as call (call.call_id)}
            <tr><td>{call.call_id}</td><td>{when(call.started_at)}</td><td class="num">{call.attempts}</td><td class="long">{call.last_error ?? ''}</td></tr>
          {/each}
        </tbody>
      </table>
    {/if}

    <h2>Database</h2>
    <dl class="facts">
      <dt>This server</dt>
      <dd>{#if data.database.primary}the primary{:else}<span class="flag">a standby</span>: changes are refused{/if}</dd>
      <dt>Address</dt>
      <dd>{data.database.address ?? 'local socket'}{data.database.port === null ? '' : `, port ${data.database.port}`}</dd>
      <dt>Version</dt>
      <dd>{data.database.version}</dd>
      <dt>Calls</dt>
      <dd>{about(data.totals.calls_about)}</dd>
      <dt>Texts</dt>
      <dd>{about(data.totals.transcripts_about)}</dd>
    </dl>
    {#if data.database.standbys.length}
      <table>
        <caption class="visually-hidden">The standbys</caption>
        <thead><tr><th scope="col">Standby</th><th scope="col">Address</th><th scope="col">State</th><th scope="col">Sync</th><th scope="col" class="num">Replay lag</th></tr></thead>
        <tbody>
          {#each data.database.standbys as standby, i (i)}
            <tr>
              <td class="long">{standby.name ?? ''}</td>
              <td class="long">{standby.address ?? ''}</td>
              <td>{standby.state ?? ''}</td>
              <td>{standby.sync_state ?? ''}</td>
              <td class="num">{standby.replay_lag_seconds === null ? '' : `${standby.replay_lag_seconds.toFixed(1)} s`}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    {:else}
      <p class="empty">No standby is connected.</p>
    {/if}

    <h2>Newest calls</h2>
    <table>
      <caption class="visually-hidden">The newest calls</caption>
      <thead><tr><th scope="col">Time</th><th scope="col">Talkgroup</th><th scope="col">Text</th></tr></thead>
      <tbody>
        {#each data.recent_calls as call (call.id)}
          <tr>
            <td>{when(call.started_at)}</td>
            <td class="long">{call.talkgroup_name ?? call.talkgroup ?? ''}{call.system ? ` · ${call.system}` : ''}</td>
            <td>{transcriptState(call.transcript_state)}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
</section>

<style>
  .muted { color: var(--muted); }
  .checks { list-style: none; margin: 0; padding: 0; display: grid; gap: 0.3rem; }
  .ok { display: inline-block; min-width: 1.4rem; text-align: center; color: var(--accent); font-weight: 700; }
  .checks .flag { min-width: 1.4rem; text-align: center; font-weight: 700; }
</style>
