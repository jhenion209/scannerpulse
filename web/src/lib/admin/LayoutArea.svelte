<script lang="ts">
  import { onMount } from 'svelte';
  import { formerPrimaryText, machineText, modeText, standbyText, type LayoutInfo } from './layout';
  import { admin, why } from './session.svelte';

  /** How often the layout is read again while the area is open. */
  const LAYOUT_EVERY_MS = 15_000;

  let data = $state<LayoutInfo | null>(null);
  let problem = $state<string | null>(null);

  /** The timer's reads are polls: they do not count as use of the session. */
  async function read(poll = false) {
    try {
      data = await admin<LayoutInfo>('GET', '/api/admin/layout', undefined, { poll });
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
  {#if problem}<p class="problem" role="alert">The layout could not be read: {problem}</p>{/if}

  {#if data}
    <p class="lead">{modeText(data.mode)}</p>

    <h2>ScannerPulse machines</h2>
    {#if data.machines.length === 0}
      <p>None has said it is up yet.</p>
    {:else}
      <dl class="facts">
        {#each data.machines as machine (machine.host)}
          {@const state = machineText(machine)}
          <!-- The plan names the second machine by its name. -->
          <dt>{machine.host}{#if machine.host === data.second} (the second){/if}</dt>
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
        this on the first ScannerPulse machine{#if data.first} ({data.first}){/if}:
      </p>
      <pre><code>{data.switchover}</code></pre>
    {/if}
  {/if}
</section>

<style>
  pre { margin: 0.4rem 0 0; padding: 0.6rem 0.8rem; border-radius: 4px; background: var(--panel, rgba(127, 127, 127, 0.1)); }
  code { font-size: 0.9rem; white-space: pre-wrap; overflow-wrap: anywhere; }
</style>
