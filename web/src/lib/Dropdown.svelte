<script lang="ts">
  import type { Snippet } from 'svelte';

  let {
    summary,
    label,
    class: className = '',
    children,
  }: {
    /** The button's text. */
    summary: string;
    /** The button's and the menu's accessible name, when the text is not enough. */
    label?: string;
    class?: string;
    /** The menu's content; it is given the function that closes the menu. */
    children: Snippet<[() => void]>;
  } = $props();

  let root: HTMLDetailsElement;
  let open = $state(false);
  const close = () => {
    root.open = false;
  };

  // What an item is given to close the menu with: it also puts the focus on
  // the menu's button first, so a dialog the item opens gives the focus back
  // there when it closes. A click or Tab away uses `close` alone and leaves
  // the focus where it went.
  const choose = () => {
    close();
    root.querySelector('summary')?.focus();
  };

  // While open, a click elsewhere closes the menu. The listener exists only
  // while a menu is open, so a long list of row menus costs nothing.
  $effect(() => {
    if (!open) return;
    const away = (event: Event) => {
      if (!root.contains(event.target as Node)) close();
    };
    window.addEventListener('click', away);
    return () => window.removeEventListener('click', away);
  });

  // The focus moving to something outside (Tab) closes it at once: a menu
  // left open could cover the control that has the focus. This listener is on
  // the menu itself, so it works from the moment the menu opens.
  function left(event: FocusEvent) {
    const to = event.relatedTarget as Node | null;
    if (root.open && to && !root.contains(to)) close();
  }

  // Escape closes it and gives the focus back to its button. The element's
  // own `open` is read: the browser reports a change a moment later, and a
  // quick key press must still count.
  function key(event: KeyboardEvent) {
    if (event.key !== 'Escape' || !root.open) return;
    event.stopPropagation();
    close();
    root.querySelector('summary')?.focus();
  }
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<details class="dropdown {className}" bind:this={root} ontoggle={() => (open = root.open)} onkeydown={key} onfocusout={left}>
  <summary aria-label={label}>{summary}</summary>
  <div class="menu" role="group" aria-label={label ?? summary}>
    {@render children(choose)}
  </div>
</details>

<style>
  .dropdown { position: relative; }
  summary {
    list-style: none; cursor: pointer; min-height: 2.25rem; display: inline-flex; align-items: center;
    padding: 0.2rem 0.6rem; border: 1px solid var(--field-border); border-radius: 4px; background: var(--bg);
  }
  summary::-webkit-details-marker { display: none; }
  .dropdown:not(.compact) summary::after { content: '▾' / ''; margin-left: 0.4rem; }
  .compact summary { min-height: 2rem; padding: 0 0.55rem; font-weight: 700; }
  .menu {
    position: absolute; z-index: 6; right: 0; margin-top: 0.25rem; min-width: 12rem; padding: 0.4rem;
    display: flex; flex-direction: column; gap: 0.1rem;
    background: var(--bg); border: 1px solid var(--field-border); border-radius: 4px;
    box-shadow: 0 4px 14px rgb(0 0 0 / 0.25);
  }
  /* On a phone the button can sit anywhere, so the menu spans the screen's
     width, just under it, instead of hanging off one side. */
  @media (max-width: 760px) {
    .menu { position: fixed; left: 0.5rem; right: 0.5rem; min-width: 0; max-height: 60vh; overflow: auto; }
  }
</style>
