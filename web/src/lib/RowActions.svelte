<script lang="ts">
  import Dropdown from './Dropdown.svelte';

  let { label, onedit, ondownload, onshare }: {
    /** Names the call, for the menu's button. */
    label: string;
    onedit: () => void;
    ondownload: () => void;
    onshare: () => void;
  } = $props();
</script>

<Dropdown summary="⋯" label={`Actions for ${label}`} class="compact row-actions">
  {#snippet children(close)}
    <button type="button" onclick={() => { close(); onedit(); }}>Edit text</button>
    <button type="button" onclick={() => { close(); ondownload(); }}>Download</button>
    <button type="button" onclick={() => { close(); onshare(); }}>Share link</button>
  {/snippet}
</Dropdown>

<style>
  button {
    text-align: left; min-height: 2.25rem; padding: 0 0.6rem; border: 0; border-radius: 4px;
    background: none; cursor: pointer; white-space: nowrap;
  }
  button:hover { background: var(--surface); }
</style>
