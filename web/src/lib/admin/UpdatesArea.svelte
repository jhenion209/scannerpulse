<script lang="ts">
  import { onMount } from 'svelte';
  import { callTime } from '../format';
  import HelperAction from './HelperAction.svelte';
  import { admin, why } from './session.svelte';
  import { siteZone } from '../status.svelte';
  import { helperText, newestText, type UpdatesInfo } from './updates';

  let data = $state<UpdatesInfo | null>(null);
  let problem = $state<string | null>(null);
  let readAt = $state<Date | null>(null);

  async function read(poll = false) {
    try {
      data = await admin<UpdatesInfo>('GET', '/api/admin/updates', undefined, { poll });
      readAt = new Date();
      problem = null;
    } catch (error) {
      problem = why(error);
    }
  }

  onMount(() => {
    void read();
    const timer = setInterval(() => void read(true), 15_000);
    return () => clearInterval(timer);
  });
</script>

<section class="area" aria-labelledby="updates-title">
  <h1 id="updates-title">Updates</h1>
  <p class="lead">
    {#if readAt}Read at {callTime(readAt.toISOString(), new Date(), siteZone())}; read again every 15 s.{:else}Reading…{/if}
  </p>
  {#if problem}<p class="problem" role="alert">The updates could not be read: {problem}</p>{/if}

  {#if data}
    {@const newest = newestText(data)}
    {@const helper = helperText(data)}
    <p class={newest.available ? 'flag' : ''}>{newest.text}</p>
    {#if newest.available && data.newest?.notes}
      <h2>What {data.newest.version} changes</h2>
      <pre><code>{data.newest.notes.trim()}</code></pre>
    {/if}

    {#if data.machines.length > 0}
      <h2>What each machine runs</h2>
      <dl class="facts">
        {#each data.machines as machine (machine.host)}
          <dt>{machine.host}</dt>
          <dd>{machine.version}</dd>
        {/each}
      </dl>
    {/if}

    <h2>Update now</h2>
    <p class={helper.ok ? '' : 'flag'}>{helper.text}</p>
    <p>
      The update reaches every machine in turn: the referee and the database machines, the second ScannerPulse
      machine, then the first. Each machine's services restart; while the machine holding the shared address
      restarts its web, a request can fail for a second or two.
    </p>
    <HelperAction
      kind="update"
      label="Update now"
      question={`Update every machine to ${data.newest?.version ?? 'the newest release'} now?`}
      allowed={newest.available}
    />
  {/if}
</section>

<style>
  pre { margin: 0.4rem 0 0; padding: 0.6rem 0.8rem; border-radius: 4px; background: var(--panel, rgba(127, 127, 127, 0.1)); }
  code { font-size: 0.9rem; white-space: pre-wrap; overflow-wrap: anywhere; }
</style>
