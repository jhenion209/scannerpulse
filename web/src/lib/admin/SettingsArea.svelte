<script lang="ts">
  import { onMount } from 'svelte';
  import SettingField from './SettingField.svelte';
  import { admin, why } from './session.svelte';
  import type { Setting } from './settings';

  let settings = $state<Setting[] | null>(null);
  let problem = $state<string | null>(null);

  async function read() {
    try {
      settings = await admin<Setting[]>('GET', '/api/admin/settings');
      problem = null;
    } catch (error) {
      problem = why(error);
    }
  }
  onMount(read);
</script>

<section class="area" aria-labelledby="settings-title">
  <h1 id="settings-title">Settings</h1>
  <p class="lead">
    Every setting, stored or not. A value is checked by the server as the part that reads it checks it, so
    a bad one is refused before it is saved. Type JSON (<code>true</code>, <code>30</code>,
    <code>["a", "b"]</code>) or plain text, such as a time zone.
  </p>
  {#if problem}<p class="problem" role="alert">The settings could not be read: {problem}</p>{/if}
  {#if settings === null}
    <p class="empty">Reading…</p>
  {:else}
    {#each settings as setting (`${setting.key}|${setting.updated_at}`)}
      <SettingField {setting} onchanged={read} />
    {/each}
  {/if}
</section>
