<script lang="ts">
  import { mine, rate } from './ratings.svelte';

  let { id, label }: {
    id: number;
    /** Names the call, for the buttons' names. */
    label: string;
  } = $props();

  const current = $derived(mine.stars[id] ?? 0);
</script>

<div class="stars" role="group" aria-label={`Rate the text of ${label}`}>
  {#each [1, 2, 3, 4, 5] as n (n)}
    <button type="button" aria-pressed={current === n} aria-label={`Rate ${n} of 5`} class:on={n <= current}
      onclick={() => rate(id, n)}>
      <svg viewBox="0 0 24 24" aria-hidden="true" focusable="false">
        <path d="M12 2.8l2.9 6.1 6.7.8-4.9 4.6 1.3 6.6L12 17.6 6 21l1.3-6.6L2.4 9.7l6.7-.8z" />
      </svg>
    </button>
  {/each}
</div>

<style>
  .stars { display: inline-flex; }
  /* A filled star is the given rating and an outline is not, so colour never tells alone. */
  button {
    display: inline-flex; align-items: center; justify-content: center;
    min-width: 1.6rem; min-height: 1.6rem; padding: 0; border: 0; background: none; cursor: pointer;
    color: var(--muted);
  }
  svg { width: 1.1rem; height: 1.1rem; fill: none; stroke: currentColor; stroke-width: 2; stroke-linejoin: round; }
  button.on { color: var(--accent-strong); }
  button.on svg { fill: currentColor; }
  @media (pointer: coarse) {
    button { min-width: 2.75rem; min-height: 2.75rem; }
    svg { width: 1.5rem; height: 1.5rem; }
  }
</style>
