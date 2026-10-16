<script lang="ts">
  import Dropdown from './Dropdown.svelte';

  let { selecting, chosenCount, busy, onexcel, oncsv, onprint, onaudio }: {
    /** Selection mode is on: the page's exports and call audio then cover the ticked calls only. */
    selecting: boolean;
    /** Calls ticked in selection mode; the page's exports cover them when there are some. */
    chosenCount: number;
    /** A file is being made on the server. */
    busy: boolean;
    onexcel: () => void;
    oncsv: () => void;
    onprint: (kind: 'pdf' | 'print') => void;
    onaudio: () => void;
  } = $props();

  const scope = $derived(selecting ? `${chosenCount} selected` : 'this page');
  // Selection on and nothing ticked: no export of the page or of the audio
  // (nothing never means everything). Excel is every match either way.
  const nothing = $derived(selecting && chosenCount === 0);
</script>

<Dropdown summary="Export" class="export">
  {#snippet children(close)}
    <button type="button" disabled={busy} onclick={() => { close(); onexcel(); }}>Excel — every match</button>
    <button type="button" disabled={nothing} onclick={() => { close(); oncsv(); }}>CSV — {scope}</button>
    <button type="button" disabled={nothing} onclick={() => { close(); onprint('pdf'); }}>PDF — {scope}</button>
    <button type="button" disabled={nothing} onclick={() => { close(); onprint('print'); }}>Print — {scope}</button>
    <button type="button" disabled={busy || nothing} onclick={() => { close(); onaudio(); }}>
      Call audio — {selecting ? `${chosenCount} selected` : 'every match'}
    </button>
    <p class="hint">PDF opens the print dialog: choose “Save as PDF”.</p>
  {/snippet}
</Dropdown>

<style>
  button {
    text-align: left; min-height: 2.25rem; padding: 0 0.6rem; border: 0; border-radius: 4px;
    background: none; cursor: pointer; white-space: nowrap;
  }
  button:hover:not(:disabled) { background: var(--surface); }
  button:disabled { opacity: 0.55; cursor: default; }
  .hint { margin: 0.3rem 0.6rem 0.1rem; font-size: 0.85rem; color: var(--muted); max-width: 16rem; }
</style>
