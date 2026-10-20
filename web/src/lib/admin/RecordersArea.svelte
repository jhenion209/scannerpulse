<script lang="ts">
  import { onMount } from 'svelte';
  import { tell } from '../notice.svelte';
  import { STATUS_EVERY_MS } from './health';
  import {
    CHANGE_LABEL,
    changedText,
    changes,
    folderProblems,
    inFolder,
    lastText,
    stateText,
    todayText,
    type Change,
    type Recorder,
    type Recorders,
  } from './recorders';
  import { admin, why } from './session.svelte';

  let data = $state<Recorders | null>(null);
  let problem = $state<string | null>(null);
  let now = $state(new Date());
  /** A refusal beside the recorder whose button asked for it. */
  let refusals = $state<Record<number, string>>({});
  /** The recorder whose removal waits for a second click. */
  let removing = $state<number | null>(null);

  let name = $state('');
  let folder = $state('');
  let whenAsked = $state(false);
  let adding = $state(false);
  let addRefusal = $state<string | null>(null);

  async function read() {
    try {
      data = await admin<Recorders>('GET', '/api/admin/recorders', undefined, { poll: true });
      now = new Date();
      problem = null;
    } catch (error) {
      problem = why(error);
    }
  }

  onMount(() => {
    void read();
    const timer = setInterval(read, STATUS_EVERY_MS);
    return () => clearInterval(timer);
  });

  async function apply(r: Recorder, change: Change) {
    delete refusals[r.id];
    try {
      await admin('POST', `/api/admin/recorders/${r.id}/${change}`);
      removing = null;
      tell(changedText(r.name, change));
      await read();
    } catch (error) {
      refusals[r.id] = why(error);
    }
  }

  async function add(event: SubmitEvent) {
    event.preventDefault();
    adding = true;
    addRefusal = null;
    try {
      await admin('POST', '/api/admin/recorders', { name, folder, when_asked: whenAsked });
      tell(`${name} is added; every running ingest picks it up within a few seconds.`);
      name = '';
      folder = '';
      whenAsked = false;
      await read();
    } catch (error) {
      addRefusal = why(error);
    } finally {
      adding = false;
    }
  }
</script>

<section class="area" aria-labelledby="recorders-title">
  <h1 id="recorders-title">Recorders</h1>
  <p class="lead">
    The SDRTrunk folders calls are taken from. A watched recorder's files are filed as they arrive; one taken in
    when asked keeps its files in its folder until you bring them in. Every file taken, and every file that
    cannot be filed, is deleted from its folder.
  </p>
  {#if problem}<p class="problem" role="alert">The recorders could not be read: {problem}</p>{/if}
  {#if data === null}
    <p class="empty">Reading…</p>
  {:else}
    <ul class="recorders">
      {#each data.recorders as r (r.id)}
        {@const troubles = folderProblems(r, data.hosts, now)}
        {@const held = r.state === 'asked' ? inFolder(r) : null}
        <li aria-labelledby={`recorder-${r.id}`}>
          <h2 id={`recorder-${r.id}`}>
            {r.name} <span class:flag={r.state !== 'watched'}>{stateText(r)}</span>
          </h2>
          <dl class="facts">
            <dt>Folder</dt><dd>{r.folder}</dd>
            <dt>Last file</dt><dd>{lastText(r, now)}</dd>
            <dt>Today</dt><dd>{todayText(r)}</dd>
            {#if held}<dt>Waiting</dt><dd>{held}</dd>{/if}
          </dl>
          {#each troubles as trouble (trouble)}<p class="problem">{trouble}</p>{/each}
          {#if removing === r.id}
            <p>Remove {r.name}? Its files stay in its folder and are no longer taken; its calls stay in the archive.</p>
            <div class="row">
              <button type="button" class="primary" onclick={() => apply(r, 'remove')}>Remove {r.name}</button>
              <button type="button" onclick={() => (removing = null)}>Keep {r.name}</button>
            </div>
          {:else}
            <div class="row">
              {#each changes(r) as change (change)}
                <button type="button" aria-label={`${CHANGE_LABEL[change]} ${r.name}`}
                  onclick={() => (change === 'remove' ? (removing = r.id) : apply(r, change))}>
                  {CHANGE_LABEL[change]}
                </button>
              {/each}
            </div>
          {/if}
          {#if refusals[r.id]}<p class="problem" role="alert">Not changed: {refusals[r.id]}</p>{/if}
        </li>
      {/each}
    </ul>
  {/if}

  <h2>Add a recorder</h2>
  <p>
    Mount the recorder's share on every ScannerPulse machine first, where ScannerPulse can read it and delete the
    files it takes. The folder is checked on this machine before it is saved; each machine then says whether it
    can read it.
  </p>
  <form class="grid" onsubmit={add}>
    <label for="recorder-name">Name</label>
    <input id="recorder-name" type="text" autocomplete="off" maxlength="40" required bind:value={name} />
    <label for="recorder-folder">Folder</label>
    <input id="recorder-folder" type="text" autocomplete="off" required placeholder="/srv/recorders/east" bind:value={folder} />
    <fieldset>
      <legend>Its files are taken</legend>
      <label><input type="radio" name="intake" value={false} bind:group={whenAsked} /> as they arrive (watched)</label>
      <label><input type="radio" name="intake" value={true} bind:group={whenAsked} /> only when asked</label>
    </fieldset>
    <div class="row"><button type="submit" class="primary" disabled={adding}>Add the recorder</button></div>
    {#if addRefusal}<p class="problem" role="alert">Not added: {addRefusal}</p>{/if}
  </form>
</section>

<style>
  .recorders { list-style: none; margin: 0; padding: 0; display: grid; gap: 0.75rem; }
  .recorders li { border: 1px solid var(--line); border-radius: 6px; padding: 0.7rem 0.9rem; background: var(--surface); }
  .recorders h2 { margin: 0 0 0.4rem; font-size: 1.05rem; overflow-wrap: anywhere; }
  .recorders h2 span { font-size: 0.85rem; font-weight: 400; }
  .facts { margin: 0.2rem 0 0.6rem; }
  .grid { display: grid; grid-template-columns: minmax(0, 1fr); gap: 0.4rem; max-width: 36rem; }
  .grid > label { font-weight: 600; }
  fieldset { border: 1px solid var(--line); border-radius: 4px; padding: 0.5rem 0.7rem; display: grid; gap: 0.4rem; min-width: 0; }
  legend { font-weight: 600; }
</style>
