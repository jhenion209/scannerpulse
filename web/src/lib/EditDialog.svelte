<script lang="ts">
  import { MAX_TEXT, textProblem } from './actions';
  import { saveTranscript, type Call } from './api';
  import { callTime } from './format';
  import { siteZone } from './status.svelte';

  let { call, onclose, onsaved }: {
    /** The call being edited; null when the dialog is closed. */
    call: Call | null;
    onclose: () => void;
    onsaved: (id: number, transcript: string, origin: string) => void;
  } = $props();

  let dialog: HTMLDialogElement;
  let text = $state('');
  let refusal = $state<string | null>(null);
  let saving = $state(false);
  const problem = $derived(textProblem(text));

  $effect(() => {
    if (call) {
      text = call.transcript ?? '';
      refusal = null;
      if (!dialog.open) dialog.showModal();
    } else if (dialog.open) dialog.close();
  });

  async function save(event: SubmitEvent) {
    event.preventDefault();
    if (!call || problem || saving) return;
    saving = true;
    try {
      const saved = await saveTranscript(call.id, text);
      onsaved(saved.id, saved.transcript, saved.origin);
      dialog.close();
    } catch (error) {
      // The server's own words: maintenance, the save limit, a call gone.
      refusal = error instanceof Error ? error.message : String(error);
    } finally {
      saving = false;
    }
  }
</script>

<dialog bind:this={dialog} onclose={onclose} aria-labelledby="edit-title">
  <form onsubmit={save}>
    <h2 id="edit-title">Edit the text</h2>
    {#if call}
      <!-- Measured against the epoch, the time always carries its date. -->
      <p class="about">{call.talkgroup_name ?? call.talkgroup} · {callTime(call.started_at, new Date(0), siteZone())}</p>
    {/if}
    <label for="edit-text">Text</label>
    <textarea id="edit-text" bind:value={text} rows="7" maxlength={MAX_TEXT} aria-describedby="edit-count"></textarea>
    <p id="edit-count" class="count">{[...text].length} / {MAX_TEXT}</p>
    {#if problem}<p class="problem" role="alert">{problem}</p>{/if}
    {#if refusal}<p class="problem" role="alert">The text was not saved: {refusal}</p>{/if}
    <div class="buttons">
      <button type="button" onclick={() => dialog.close()}>Cancel</button>
      <button type="submit" class="primary" disabled={!!problem || saving}>{saving ? 'Saving…' : 'Save'}</button>
    </div>
  </form>
</dialog>

<style>
  dialog {
    width: min(36rem, calc(100vw - 2rem)); border: 1px solid var(--field-border); border-radius: 6px;
    background: var(--bg); color: var(--text); padding: 1rem;
  }
  dialog::backdrop { background: rgb(0 0 0 / 0.45); }
  h2 { margin: 0 0 0.3rem; font-size: 1.1rem; }
  .about { margin: 0 0 0.8rem; color: var(--muted); }
  label { display: block; font-weight: 600; margin-bottom: 0.3rem; }
  textarea {
    width: 100%; font: inherit; color: inherit; background: var(--bg); padding: 0.5rem;
    border: 1px solid var(--field-border); border-radius: 4px; resize: vertical;
  }
  .count { margin: 0.2rem 0 0; font-size: 0.85rem; color: var(--muted); text-align: right; }
  .problem { margin: 0.5rem 0 0; padding: 0.4rem 0.6rem; border-radius: 4px; background: var(--warn-bg); color: var(--warn-text); }
  .buttons { display: flex; justify-content: flex-end; gap: 0.5rem; margin-top: 0.9rem; }
  .buttons button {
    min-height: 2.25rem; padding: 0 0.9rem; border: 1px solid var(--field-border); border-radius: 4px;
    background: var(--bg); cursor: pointer;
  }
  .buttons .primary { background: var(--accent); border-color: var(--accent); color: var(--bg); font-weight: 600; }
  .buttons button:disabled { opacity: 0.55; cursor: default; }
</style>
