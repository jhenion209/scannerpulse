<script lang="ts">
  // The last step of a job that moves or deletes files: it starts only with
  // the talkgroup's name typed exactly (the server checks it too).
  let { name, action, start }: { name: string; action: string; start: (confirm: string) => Promise<void> } = $props();
  const id = $props.id();
  let typed = $state('');
  let busy = $state(false);

  async function go() {
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
    <button type="button" class="primary" disabled={typed !== name || busy} onclick={go}>{action}</button>
  </div>
</div>

<style>
  .confirm { display: grid; gap: 0.3rem; margin: 0.5rem 0; }
</style>
