<script lang="ts">
  import Logo from './Logo.svelte';
  import type { Theme } from './prefs';
  import StatusChips from './StatusChips.svelte';
  import ThemeSwitch from './ThemeSwitch.svelte';

  let { railOpen, words = '', theme = $bindable('auto'), ontoggle, onsearch }: {
    /** Whether the filter rail (or, on a phone, its menu) is open. */
    railOpen: boolean;
    /** The words a link brought, to start the search box with. */
    words?: string;
    theme?: Theme;
    ontoggle: () => void;
    /** Opens the search panel, with the words typed when there are some. */
    onsearch: (words: string) => void;
  } = $props();

  // svelte-ignore state_referenced_locally
  let text = $state(words);

  /** The skip link lands on the first call's play bar, or else the list's
   * first enabled control: the list itself takes no focus, so a click in it never
   * moves the focus away from an open menu. */
  function skip(event: MouseEvent) {
    const target =
      document.querySelector<HTMLElement>('#calls tbody button') ??
      document.querySelector<HTMLElement>('#calls button:not(:disabled), #calls input:not(:disabled), #calls select:not(:disabled)');
    if (!target) return;
    event.preventDefault();
    target.focus();
  }

  function submit(event: SubmitEvent) {
    event.preventDefault();
    onsearch(text.trim());
  }
</script>

<header class="bar">
  <a class="skip" href="#calls" onclick={skip}>Skip to the calls</a>
  <button type="button" class="menu" onclick={ontoggle} aria-controls="filters" aria-expanded={railOpen}>
    Filters
  </button>
  <span class="name"><Logo />ScannerPulse</span>
  <form class="find" role="search" aria-label="Search calls" onsubmit={submit}>
    <input type="search" aria-label="Words to search for" placeholder="Search calls" bind:value={text} />
    <button type="submit">Search</button>
  </form>
  <span class="spacer"></span>
  <StatusChips />
  <ThemeSwitch bind:theme />
  <a class="admin" href="/admin">Admin</a>
</header>

<style>
  .bar {
    grid-area: bar; display: flex; flex-wrap: wrap; gap: 0.75rem; row-gap: 0.4rem; align-items: center;
    padding: max(0.5rem, var(--safe-top)) max(1rem, var(--safe-right)) 0.5rem max(1rem, var(--safe-left));
    background: var(--bar); color: var(--bar-text);
  }
  .menu {
    background: none; color: var(--bar-text); border: 1px solid currentColor; border-radius: 4px;
    padding: 0.25rem 0.6rem; min-height: 2.25rem; cursor: pointer;
  }
  .name { font-weight: 700; color: var(--accent-strong); display: inline-flex; align-items: center; gap: 0.35rem; }
  .find { display: flex; gap: 0.3rem; }
  .find input {
    min-height: 2.25rem; width: 14rem; padding: 0.2rem 0.5rem; border-radius: 4px;
    border: 1px solid var(--bar-text); background: var(--bg); color: var(--text); font: inherit;
  }
  .find button {
    min-height: 2.25rem; padding: 0 0.7rem; border-radius: 4px; border: 1px solid var(--bar-text);
    background: none; color: var(--bar-text); font: inherit; cursor: pointer;
  }
  .spacer { flex: 1; }
  /* Out of sight until the keyboard reaches it: the first stop of the page. */
  .skip { position: absolute; left: -999px; top: 0.5rem; z-index: 40; padding: 0.4rem 0.7rem; border-radius: 4px; background: var(--bg); color: var(--accent); }
  .skip:focus { left: 0.5rem; }
  .admin { color: var(--bar-text); }
  @media print { .bar { display: none !important; } }
  /* A phone keeps the Search button; the panel has the words field. */
  @media (max-width: 760px) { .find input { display: none; } }
</style>
