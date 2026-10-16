<script lang="ts">
  import { COLUMNS, toggleColumn, type ColumnId } from './columns';

  let {
    columns,
    customised,
    onchange,
    onreset,
  }: {
    columns: ColumnId[];
    /** The listener has chosen columns: "Reset" has something to undo. */
    customised: boolean;
    onchange: (columns: ColumnId[]) => void;
    onreset: () => void;
  } = $props();

  let root: HTMLDetailsElement;

  // The menu closes with Escape (focus goes back to its button), with a click
  // elsewhere, and when the focus moves on (Tab): a menu left open could cover
  // the control that has the focus. The element's own `open` is read, not a
  // copy of it: the browser reports a change later, and a quick key press
  // must still count.
  function key(event: KeyboardEvent) {
    if (event.key !== 'Escape' || !root.open || !root.contains(document.activeElement)) return;
    root.open = false;
    root.querySelector('summary')?.focus();
  }

  function outside(event: Event) {
    if (root.open && !root.contains(event.target as Node)) root.open = false;
  }
</script>

<svelte:window onclick={outside} onfocusin={outside} onkeydown={key} />

<details class="columns" bind:this={root}>
  <summary>Columns</summary>
  <div class="menu" role="group" aria-label="Columns">
    {#each COLUMNS as column (column.id)}
      <label>
        <input type="checkbox" checked={columns.includes(column.id)}
          onchange={() => onchange(toggleColumn(columns, column.id))} />
        {column.label}
      </label>
    {/each}
    <button type="button" class="reset" onclick={onreset} disabled={!customised}>Reset to default</button>
  </div>
</details>

<style>
  .columns { position: relative; }
  summary {
    list-style: none; cursor: pointer; min-height: 2.25rem; display: inline-flex; align-items: center;
    padding: 0.2rem 0.6rem; border: 1px solid var(--field-border); border-radius: 4px; background: var(--bg);
  }
  summary::-webkit-details-marker { display: none; }
  summary::after { content: '▾' / ''; margin-left: 0.4rem; }
  .menu {
    position: absolute; z-index: 6; right: 0; margin-top: 0.25rem; min-width: 12rem; padding: 0.4rem;
    display: flex; flex-direction: column; gap: 0.1rem;
    background: var(--bg); border: 1px solid var(--field-border); border-radius: 4px;
    box-shadow: 0 4px 14px rgb(0 0 0 / 0.25);
  }
  label { display: flex; gap: 0.5rem; align-items: center; min-height: 2rem; padding: 0 0.3rem; white-space: nowrap; }
  .reset {
    margin-top: 0.3rem; min-height: 2.25rem; border: 1px solid var(--field-border); border-radius: 4px;
    background: var(--bg); cursor: pointer;
  }
  .reset:disabled { cursor: default; opacity: 0.55; }
  /* On a phone the button can sit anywhere in the wrapped bar, so the menu
     spans the screen's width, just under it, instead of hanging off one side. */
  @media (max-width: 760px) {
    .menu { position: fixed; left: 0.5rem; right: 0.5rem; min-width: 0; max-height: 60vh; overflow: auto; }
  }
</style>
