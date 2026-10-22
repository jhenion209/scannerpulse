<script lang="ts">
  // The last step of a job that moves or deletes files: it starts only with
  // the word asked for typed exactly (the server checks it too). With `ask`,
  // the button opens a confirmation pop-up first.
  import ConfirmDialog from './ConfirmDialog.svelte';

  let {
    name,
    action,
    start,
    ask,
    danger = false,
  }: {
    name: string;
    action: string;
    start: (confirm: string) => Promise<void>;
    ask?: { title: string; text: string; action: string };
    /** Red: a change that deletes for good. */
    danger?: boolean;
  } = $props();
  const id = $props.id();
  let typed = $state('');
  let busy = $state(false);
  let asking = $state(false);

  async function go() {
    asking = false;
    busy = true;
    try {
      await start(typed);
    } finally {
      busy = false;
    }
  }
</script>

<div class="confirm">
  <label for={id}>Type <strong>{name}</strong> to confirm</label>
  <div class="row">
    <input {id} type="text" autocomplete="off" autocapitalize="off" autocorrect="off" spellcheck="false" bind:value={typed} />
    <button type="button" class={danger ? 'danger' : 'primary'} disabled={typed !== name || busy} onclick={() => (ask ? (asking = true) : go())}>{action}</button>
  </div>
</div>
{#if asking && ask}
  <ConfirmDialog title={ask.title} text={ask.text} action={ask.action} {danger} confirm={go} cancel={() => (asking = false)} />
{/if}

<style>
  .confirm { display: grid; grid-template-columns: minmax(0, 1fr); gap: 0.3rem; margin: 0.5rem 0; }
  label { overflow-wrap: anywhere; }
</style>
