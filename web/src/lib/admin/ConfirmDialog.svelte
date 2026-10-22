<script lang="ts">
  // The last question before a change that cannot be undone: a modal pop-up
  // whose Cancel has the focus, so Enter or Escape cancels.
  let {
    title,
    text,
    action,
    confirm,
    cancel,
  }: { title: string; text: string; action: string; confirm: () => void; cancel: () => void } = $props();
  const id = $props.id();
  let dialog: HTMLDialogElement;
  let keep: HTMLButtonElement;

  $effect(() => {
    dialog.showModal();
    keep.focus();
  });
</script>

<dialog bind:this={dialog} aria-labelledby={`${id}-title`} aria-describedby={`${id}-text`}
  oncancel={(event) => { event.preventDefault(); cancel(); }}>
  <h2 id={`${id}-title`}>{title}</h2>
  <p id={`${id}-text`}>{text} <strong>There is no undo.</strong></p>
  <div class="row">
    <button type="button" bind:this={keep} onclick={cancel}>Cancel</button>
    <button type="button" class="primary" onclick={confirm}>{action}</button>
  </div>
</dialog>

<style>
  dialog { max-width: min(26rem, calc(100vw - 2rem)); border: 1px solid var(--line); border-radius: 8px; padding: 1rem 1.1rem; background: var(--surface); color: var(--text); overflow-wrap: anywhere; }
  dialog::backdrop { background: rgb(0 0 0 / 0.45); }
  h2 { margin: 0 0 0.5rem; font-size: 1.05rem; }
  .row { display: flex; flex-wrap: wrap; gap: 0.5rem; justify-content: space-between; }
</style>
