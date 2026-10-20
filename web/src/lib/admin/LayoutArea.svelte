<script lang="ts">
  import { onMount } from 'svelte';
  import { callTime } from '../format';
  import ConvertGuide from './ConvertGuide.svelte';
  import HelperAction from './HelperAction.svelte';
  import { firstMachine, formerPrimaryText, machineLabel, machineText, modeText, movingText, standbyText, type LayoutInfo } from './layout';
  import { admin, why } from './session.svelte';
  import { siteZone } from '../status.svelte';

  /** How often the layout is read again while the area is open. */
  const LAYOUT_EVERY_MS = 15_000;

  let data = $state<LayoutInfo | null>(null);
  let problem = $state<string | null>(null);
  /** When the layout on screen was read: a failed read keeps it, undated otherwise. */
  let readAt = $state<Date | null>(null);

  /** The timer's reads are polls: they do not count as use of the session. */
  async function read(poll = false) {
    try {
      data = await admin<LayoutInfo>('GET', '/api/admin/layout', undefined, { poll });
      readAt = new Date();
      problem = null;
    } catch (error) {
      problem = why(error);
    }
  }

  onMount(() => {
    void read();
    const timer = setInterval(() => void read(true), LAYOUT_EVERY_MS);
    return () => clearInterval(timer);
  });

  const holder = $derived(data?.machines.find((m) => m.holds_shared)?.host ?? null);
</script>

<section class="area" aria-labelledby="layout-title">
  <h1 id="layout-title">Layout</h1>
  <p class="lead">
    {#if readAt}Read at {callTime(readAt.toISOString(), new Date(), siteZone())}; read again every 15 s.{:else}Reading…{/if}
  </p>
  {#if problem}<p class="problem" role="alert">The layout could not be read: {problem}</p>{/if}

  {#if data}
    <p class="lead">{data.moving ? movingText : modeText(data.mode)}</p>
    <ConvertGuide one={data.mode === 'one'} />

    <h2>ScannerPulse machines</h2>
    {#if data.machines.length === 0}
      <p>None has said it is up yet.</p>
    {:else}
      <dl class="facts">
        {#each data.machines as machine (machine.host)}
          {@const state = machineText(machine)}
          <!-- The plan names the second machine by its name. -->
          <dt>{machineLabel(machine.host, data.second)}</dt>
          <dd>
            <span class={state.ok ? '' : 'flag'}>{state.text}</span>{#if machine.holds_shared}; holds the shared address{/if}{#if machine.holds_lease}; files and transcribes the calls{/if}
          </dd>
        {/each}
      </dl>
    {/if}
    {#if data.shared}
      <p>
        The shared address is {data.shared}:
        {#if holder}{holder} holds it.{:else}<span class="flag">no machine holds it.</span>{/if}
      </p>
    {/if}

    {#if data.database.length > 0}
      <h2>Database machines</h2>
      <dl class="facts">
        {#each data.database as node (node.host)}
          <dt>{node.host}</dt>
          <dd>
            {#if node.role === null}
              <span class="flag">does not answer: {node.error}</span>
            {:else if node.role === 'primary'}
              the primary
            {:else if node.role === 'former_primary'}
              <span class="flag">{formerPrimaryText}</span>
            {:else}
              {@const behind = standbyText(node)}
              the standby, <span class={behind.ok ? '' : 'flag'}>{behind.text}</span>
            {/if}
          </dd>
        {/each}
      </dl>
    {/if}

    {#if data.referee}
      <h2>Referee</h2>
      <dl class="facts">
        <dt>{data.referee.host}</dt>
        <dd>
          {#if data.referee.answers}answers on its port{:else}<span class="flag">does not answer: {data.referee.error}</span> (while it is down, nothing is promoted){/if}
        </dd>
      </dl>
    {/if}

    {#if data.switchover}
      <h2>Switchover</h2>
      <p>
        To swap the database machines' roles while both are healthy (writes pause for seconds; nothing is lost), run
        this on {firstMachine(data.first)}:
      </p>
      <pre><code>{data.switchover}</code></pre>
      <p>Or ask the root helper there to do it now:</p>
      <HelperAction
        kind="switchover"
        label="Switch the roles now"
        question="Make the standby the primary, and the primary its standby, now? Writes pause for seconds."
      />
    {/if}
  {/if}
</section>

<style>
  pre { margin: 0.4rem 0 0; padding: 0.6rem 0.8rem; border-radius: 4px; background: var(--panel, rgba(127, 127, 127, 0.1)); }
  code { font-size: 0.9rem; white-space: pre-wrap; overflow-wrap: anywhere; }
</style>
