<script lang="ts">
  import { onMount } from 'svelte';
  import { callTime } from '../format';
  import { siteZone } from '../status.svelte';
  import { admin, why } from './session.svelte';

  interface Viewer {
    host: string;
    address: string;
    /** The address is on this network. */
    local: boolean;
    user_agent: string;
    /** The admin who opened the page, or null. */
    admin: string | null;
    connected_at: string;
  }

  /** How often the open pages are read again; the server refreshes them as often. */
  const VIEWERS_EVERY_MS = 30_000;

  let viewers = $state<Viewer[] | null>(null);
  let problem = $state<string | null>(null);

  /** The timer's reads are polls: they do not count as use of the session. */
  async function read(poll = false) {
    try {
      viewers = await admin<Viewer[]>('GET', '/api/admin/viewers', undefined, { poll });
      problem = null;
    } catch (error) {
      problem = why(error);
    }
  }

  onMount(() => {
    void read();
    const timer = setInterval(() => void read(true), VIEWERS_EVERY_MS);
    return () => clearInterval(timer);
  });
</script>

<section class="area" aria-labelledby="viewers-title">
  <h1 id="viewers-title">Viewers</h1>
  <div class="row lead">
    <span>Every page open on the calls now, on every host; a page leaves the list when it closes. Read every 30 s.</span>
    <button type="button" onclick={() => read()}>Read now</button>
  </div>
  {#if problem}<p class="problem" role="alert">The viewers could not be read: {problem}</p>{/if}
  {#if viewers === null}
    <p class="empty">Reading…</p>
  {:else if viewers.length === 0}
    <p class="empty">No page is open.</p>
  {:else}
    <table>
      <caption class="visually-hidden">The open pages</caption>
      <thead><tr><th scope="col">Since</th><th scope="col">Address</th><th scope="col">Admin</th><th scope="col">Browser</th><th scope="col">Host</th></tr></thead>
      <tbody>
        {#each viewers as viewer, i (i)}
          <tr>
            <td>{callTime(viewer.connected_at, new Date(), siteZone())}</td>
            <td class="long">{viewer.address}{viewer.local ? ' (local)' : ''}</td>
            <td class="long">{viewer.admin ?? '—'}</td>
            <td class="agent long">{viewer.user_agent}</td>
            <td class="long">{viewer.host}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
</section>

<style>
  .agent { color: var(--muted); }
</style>
