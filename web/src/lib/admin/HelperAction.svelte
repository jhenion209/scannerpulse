<script lang="ts">
  // One thing an admin asks the root helper for (an update, a switchover):
  // its button, a confirming step, and the latest such request with what the
  // helper said, read again every 2 s while it is open. Through an update's
  // restarts a read fails for a moment: the last one stays on screen.
  import { onMount } from 'svelte';
  import { admin, why } from './session.svelte';
  import { canAsk, requestState, type UpdatesInfo } from './updates';

  let {
    kind,
    label,
    question,
    allowed = true,
  }: { kind: 'update' | 'switchover'; label: string; question: string; allowed?: boolean } = $props();

  let info = $state<UpdatesInfo | null>(null);
  let problem = $state<string | null>(null);
  let asking = $state(false);
  let busy = $state(false);

  const request = $derived(info?.requests[kind] ?? null);

  async function read(poll = true) {
    try {
      info = await admin<UpdatesInfo>('GET', '/api/admin/updates', undefined, { poll });
      problem = null;
    } catch (error) {
      problem = info?.open
        ? 'the site did not answer: it may be restarting for the update; reading again shortly'
        : why(error);
    }
  }

  async function ask() {
    busy = true;
    try {
      await admin('POST', '/api/admin/helper', { kind });
      asking = false;
      await read(false);
    } catch (error) {
      problem = why(error);
    } finally {
      busy = false;
    }
  }

  onMount(() => {
    void read(false);
    let timer: ReturnType<typeof setTimeout>;
    const next = () => {
      timer = setTimeout(async () => {
        await read();
        next();
      }, info?.open ? 2_000 : 15_000);
    };
    next();
    return () => clearTimeout(timer);
  });
</script>

{#if info && allowed}
  {#if asking}
    <div class="confirm" role="group" aria-label={label}>
      <p>{question}</p>
      <div class="row">
        <button type="button" class="primary" disabled={busy || !canAsk(info)} onclick={ask}>{label}</button>
        <button type="button" disabled={busy} onclick={() => (asking = false)}>Cancel</button>
      </div>
    </div>
  {:else}
    <button type="button" disabled={!canAsk(info)} onclick={() => (asking = true)}>{label}</button>
  {/if}
{/if}
{#if problem}<p class="problem" role="alert">{problem}</p>{/if}
{#if request}
  {@const state = requestState(request)}
  <h3>The latest {kind}, asked by {request.requested_by}</h3>
  <p class={state.ok ? '' : 'flag'}>{state.text}</p>
  {#if request.said}<pre aria-live="polite"><code>{request.said.trimEnd()}</code></pre>{/if}
{/if}

<style>
  .confirm { display: grid; gap: 0.4rem; margin: 0.5rem 0; }
  .row { display: flex; gap: 0.5rem; flex-wrap: wrap; }
  pre { margin: 0.4rem 0 0; padding: 0.6rem 0.8rem; border-radius: 4px; background: var(--panel, rgba(127, 127, 127, 0.1)); max-height: 20rem; overflow: auto; }
  code { font-size: 0.9rem; white-space: pre-wrap; overflow-wrap: anywhere; }
</style>
