<script lang="ts">
  import { passwordProblem } from './accounts';
  import { why } from './session.svelte';

  let { id, withName = false, submitLabel, onsubmit, oncancel }: {
    /** Makes the fields' ids unique on the page. */
    id: string;
    /** Ask for the account's name too (a new account). */
    withName?: boolean;
    submitLabel: string;
    /** Saves; throws the server's refusal, which the form shows. */
    onsubmit: (login: string, password: string) => Promise<void>;
    oncancel?: () => void;
  } = $props();

  let login = $state('');
  let password = $state('');
  let again = $state('');
  let refusal = $state<string | null>(null);
  let busy = $state(false);

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    refusal = passwordProblem(password, again);
    if (refusal || busy) return;
    busy = true;
    try {
      await onsubmit(login.trim(), password);
      login = '';
    } catch (error) {
      refusal = why(error);
    } finally {
      // A password never stays in the page once it is sent, whatever the answer.
      password = again = '';
      busy = false;
    }
  }
</script>

<form class="password" onsubmit={submit}>
  {#if withName}
    <label for={`${id}-login`}>Name</label>
    <input id={`${id}-login`} autocomplete="off" required bind:value={login} />
  {/if}
  <label for={`${id}-password`}>Password</label>
  <input id={`${id}-password`} type="password" autocomplete="new-password" required bind:value={password} />
  <label for={`${id}-again`}>Password again</label>
  <input id={`${id}-again`} type="password" autocomplete="new-password" required bind:value={again} />
  <div class="row">
    <button type="submit" class="primary" disabled={busy}>{submitLabel}</button>
    {#if oncancel}<button type="button" onclick={oncancel}>Cancel</button>{/if}
  </div>
  {#if refusal}<p class="problem" role="alert">Not saved: {refusal}</p>{/if}
</form>

<style>
  .password { display: grid; gap: 0.3rem; max-width: 24rem; margin: 0.5rem 0; }
  label { font-weight: 600; margin-top: 0.3rem; }
  .row { margin-top: 0.6rem; }
</style>
