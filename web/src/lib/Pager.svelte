<script lang="ts">
  import ColumnMenu from './ColumnMenu.svelte';
  import type { Snippet } from 'svelte';
  import type { ColumnId } from './columns';
  import { pageCount } from './paging';
  import { PAGE_SIZES } from './prefs';

  let {
    index,
    hasNext,
    total,
    capped,
    size = $bindable(),
    showAll = $bindable(),
    selecting = $bindable(false),
    chosenCount = 0,
    extra,
    columns,
    customised,
    oncolumns,
    onresetcolumns,
    ongo,
  }: {
    index: number;
    hasNext: boolean;
    total: number | null;
    capped: boolean;
    size: number;
    showAll: boolean;
    selecting?: boolean;
    chosenCount?: number;
    /** More controls for the bar (the Export menu). */
    extra?: Snippet;
    columns: ColumnId[];
    customised: boolean;
    oncolumns: (columns: ColumnId[]) => void;
    onresetcolumns: () => void;
    ongo: (page: number) => void;
  } = $props();

  const count = $derived(total === null ? null : pageCount(total, capped, size));
  let typed = $state('');
  $effect(() => {
    typed = String(index + 1);
  });

  function jump(event: SubmitEvent) {
    event.preventDefault();
    const page = Number(typed);
    if (Number.isInteger(page) && page >= 1) ongo(page - 1);
    else typed = String(index + 1);
  }
</script>

<nav class="pager" aria-label="Pages">
  <span class="total">
    {#if total !== null}{total.toLocaleString()}{capped ? '+' : ''} calls{/if}
  </span>
  <button type="button" onclick={() => ongo(0)} disabled={index === 0}>First</button>
  <button type="button" onclick={() => ongo(index - 1)} disabled={index === 0}>Previous</button>
  <form onsubmit={jump}>
    <label>
      Page
      <input type="text" inputmode="numeric" size="4" bind:value={typed} aria-label="Page number" />
    </label>
    {#if count}<span>of {count.pages.toLocaleString()}{count.more ? '+' : ''}</span>{/if}
  </form>
  <button type="button" onclick={() => ongo(index + 1)} disabled={!hasNext}>Next</button>
  <label><input type="checkbox" class="check" bind:checked={showAll} /> Show all text</label>
  <button type="button" class="select" aria-pressed={selecting} onclick={() => (selecting = !selecting)}>Select calls</button>
  {#if selecting}<span class="chosen" aria-live="polite">{chosenCount} selected</span>{/if}
  {@render extra?.()}
  <ColumnMenu {columns} {customised} onchange={oncolumns} onreset={onresetcolumns} />
  <label class="size">
    Rows
    <select bind:value={size}>
      {#each PAGE_SIZES as choice (choice)}<option value={choice}>{choice}</option>{/each}
    </select>
  </label>
</nav>

<style>
  .pager { display: flex; flex-wrap: wrap; gap: 0.5rem; align-items: center; padding: 0.5rem 0.75rem; border-bottom: 1px solid var(--line); }
  .total { margin-right: auto; color: var(--muted); }
  form, label { display: flex; gap: 0.35rem; align-items: center; margin: 0; }
  button, input, select {
    min-height: 2.25rem; padding: 0.2rem 0.6rem; border: 1px solid var(--field-border); border-radius: 4px; background: var(--bg);
  }
  button { cursor: pointer; }
  button:disabled { cursor: default; opacity: 0.55; }
  input[type='text'] { width: 4.5rem; text-align: right; }
  .select[aria-pressed='true'] { background: var(--accent); border-color: var(--accent); color: var(--bg); font-weight: 600; }
  .chosen { color: var(--muted); }
  .check { min-height: 0; }
</style>
