<script lang="ts">
  // The order of the systems' groups in the main page's talkgroup list: the
  // setting web.system_order, set by moving the systems up and down.
  import { onMount } from 'svelte';
  import { fetchFilterChoices } from '../api';
  import { admin, why } from './session.svelte';
  import { moved } from './settings';

  let { onsaved }: { onsaved: () => void } = $props();

  /** The order the server gives now, and the one on screen. */
  let saved = $state<string[] | null>(null);
  let order = $state<string[]>([]);
  let problem = $state<string | null>(null);
  let answer = $state<{ ok: boolean; text: string } | null>(null);
  let busy = $state(false);
  const changed = $derived(saved !== null && order.join('\n') !== saved.join('\n'));

  onMount(async () => {
    try {
      const choices = await fetchFilterChoices();
      saved = choices.systems.map((s) => s.name);
      order = [...saved];
    } catch (error) {
      problem = why(error);
    }
  });

  function move(index: number, by: -1 | 1) {
    order = moved(order, index, by);
    answer = null;
  }

  async function save() {
    busy = true;
    answer = null;
    try {
      await admin('PUT', '/api/admin/settings/web.system_order', { value: order });
      saved = [...order];
      answer = { ok: true, text: 'Saved. The talkgroup list shows the systems in this order.' };
      onsaved();
    } catch (error) {
      answer = { ok: false, text: `Not saved: ${why(error)}` };
    } finally {
      busy = false;
    }
  }
</script>

<h2 id="system-order-title">Order of the systems</h2>
<p>The order of the systems' groups in the main page's talkgroup list. A system added later comes after these, by name.</p>
{#if problem}
  <p class="problem" role="alert">The systems could not be read: {problem}</p>
{:else if saved === null}
  <p class="empty">Reading…</p>
{:else if order.length < 2}
  <p class="empty">There is one system, so there is nothing to order.</p>
{:else}
  <ol class="systems" aria-labelledby="system-order-title">
    {#each order as name, index (name)}
      <li>
        <span class="name">{name}</span>
        <button type="button" aria-label={`Move ${name} up`} disabled={index === 0} onclick={() => move(index, -1)}>Up</button>
        <button type="button" aria-label={`Move ${name} down`} disabled={index === order.length - 1} onclick={() => move(index, 1)}>Down</button>
      </li>
    {/each}
  </ol>
  <div class="row"><button type="button" class="primary" disabled={!changed || busy} onclick={save}>Save the order</button></div>
  {#if answer}
    {#if answer.ok}<p role="status">{answer.text}</p>{:else}<p class="problem" role="alert">{answer.text}</p>{/if}
  {/if}
{/if}

<style>
  .systems { list-style: none; margin: 0 0 0.6rem; padding: 0; max-width: 28rem; display: grid; gap: 0.35rem; counter-reset: system; }
  .systems li { display: flex; align-items: center; gap: 0.5rem; padding: 0.3rem 0; border-bottom: 1px solid var(--line); counter-increment: system; }
  .systems li::before { content: counter(system) '.'; color: var(--muted); min-width: 1.5rem; font-variant-numeric: tabular-nums; }
  .name { flex: 1; min-width: 0; overflow-wrap: anywhere; font-weight: 600; }
</style>
