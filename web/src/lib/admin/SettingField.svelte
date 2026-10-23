<script lang="ts">
  import { callTime } from '../format';
  import { tell } from '../notice.svelte';
  import { siteZone } from '../status.svelte';
  import { admin, can, isSuper, why } from './session.svelte';
  import { parseValue, settingFunction, showValue, type Setting } from './settings';

  let { setting, onchanged }: {
    setting: Setting;
    /** After a save or a removal: the list is read again. */
    onchanged: () => void;
  } = $props();

  const stored = $derived(setting.updated_at !== null);
  // svelte-ignore state_referenced_locally
  let text = $state(stored ? showValue(setting.value) : '');
  let refusal = $state<string | null>(null);
  let busy = $state(false);
  const id = $derived(`setting-${setting.key.replaceAll('.', '-')}`);
  /** Whether this admin may change it: otherwise it is only shown. */
  const mine = $derived.by(() => {
    const fn = settingFunction(setting.key);
    return fn === null ? isSuper() : can(fn);
  });

  async function change(remove: boolean) {
    refusal = null;
    const typed = parseValue(text);
    if (!remove && typed === null) {
      refusal = 'Type a value first, or remove the setting.';
      return;
    }
    busy = true;
    try {
      const path = `/api/admin/settings/${encodeURIComponent(setting.key)}`;
      if (remove) await admin('DELETE', path);
      else await admin('PUT', path, typed);
      tell(remove ? `Removed ${setting.key}.` : `Saved ${setting.key}.`);
      if (remove) text = '';
      onchanged();
    } catch (error) {
      refusal = why(error);
    } finally {
      busy = false;
    }
  }
</script>

<form class="setting" onsubmit={(event) => { event.preventDefault(); void change(false); }}>
  <label for={id}><code>{setting.key}</code></label>
  <p class="when">
    {#if !setting.known}No part reads this setting: it can only be removed.
    {:else if stored}Changed {callTime(setting.updated_at!, new Date(), siteZone())}
    {:else}Not set{/if}
  </p>
  <div class="row">
    <input {id} bind:value={text} disabled={!setting.known || !mine} aria-describedby={refusal ? `${id}-refusal` : undefined} />
    {#if setting.known && mine}<button type="submit" disabled={busy}>Save</button>{/if}
    {#if stored && mine}<button type="button" disabled={busy} onclick={() => change(true)}>Remove</button>{/if}
  </div>
  {#if refusal}<p class="problem" role="alert" id={`${id}-refusal`}>Not saved: {refusal}</p>{/if}
</form>

<style>
  .setting { padding: 0.6rem 0; border-bottom: 1px solid var(--line); }
  label { font-weight: 600; overflow-wrap: anywhere; }
  .when { margin: 0.1rem 0 0.35rem; color: var(--muted); }
  input { flex: 1 1 16rem; min-width: 0; }
</style>
