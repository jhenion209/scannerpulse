<script lang="ts">
  import { COLUMNS, toggleColumn, type ColumnId } from './columns';
  import Dropdown from './Dropdown.svelte';

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
</script>

<Dropdown summary="Columns" class="columns">
  {#snippet children()}
    {#each COLUMNS as column (column.id)}
      <label>
        <input type="checkbox" checked={columns.includes(column.id)}
          onchange={() => onchange(toggleColumn(columns, column.id))} />
        {column.label}
      </label>
    {/each}
    <button type="button" class="reset" onclick={onreset} disabled={!customised}>Reset to default</button>
  {/snippet}
</Dropdown>

<style>
  label { display: flex; gap: 0.5rem; align-items: center; min-height: 2rem; padding: 0 0.3rem; white-space: nowrap; }
  .reset {
    margin-top: 0.3rem; min-height: 2.25rem; border: 1px solid var(--field-border); border-radius: 4px;
    background: var(--bg); cursor: pointer;
  }
  .reset:disabled { cursor: default; opacity: 0.55; }
</style>
