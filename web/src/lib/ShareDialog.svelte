<script lang="ts">
  let { url, onclose }: {
    /** The link to show; null when the dialog is closed. */
    url: string | null;
    onclose: () => void;
  } = $props();

  let dialog: HTMLDialogElement;
  let field: HTMLInputElement;

  $effect(() => {
    if (url) {
      if (!dialog.open) dialog.showModal();
      field.select();
    } else if (dialog.open) dialog.close();
  });
</script>

<!-- Shown when the browser will not copy to the clipboard (a page served over
     plain HTTP on a LAN address is not allowed to): the link is selected,
     ready to copy by hand. -->
<dialog bind:this={dialog} onclose={onclose} aria-labelledby="share-title">
  <h2 id="share-title">Link to this call</h2>
  <label for="share-url">Copy the link</label>
  <input id="share-url" bind:this={field} type="text" readonly value={url ?? ''} />
  <div class="buttons"><button type="button" onclick={() => dialog.close()}>Close</button></div>
</dialog>

<style>
  dialog {
    width: min(32rem, calc(100vw - 2rem)); border: 1px solid var(--field-border); border-radius: 6px;
    background: var(--bg); color: var(--text); padding: 1rem;
  }
  dialog::backdrop { background: rgb(0 0 0 / 0.45); }
  h2 { margin: 0 0 0.6rem; font-size: 1.1rem; }
  label { display: block; font-weight: 600; margin-bottom: 0.3rem; }
  input {
    width: 100%; font: inherit; color: inherit; background: var(--bg); padding: 0.4rem 0.5rem;
    border: 1px solid var(--field-border); border-radius: 4px;
  }
  .buttons { display: flex; justify-content: flex-end; margin-top: 0.9rem; }
  button {
    min-height: 2.25rem; padding: 0 0.9rem; border: 1px solid var(--field-border); border-radius: 4px;
    background: var(--bg); cursor: pointer;
  }
</style>
