<script lang="ts">
  import { onMount } from 'svelte';
  import { callTime, counted, seconds, source } from '../format';
  import { tell } from '../notice.svelte';
  import { siteZone } from '../status.svelte';
  import { clockSentence, STATUS_EVERY_MS } from './health';
  import {
    CHANGE_LABEL,
    changedText,
    changes,
    folderProblems,
    inFolder,
    lastText,
    nameDecidedText,
    previewText,
    seenText,
    stateText,
    systemDecidedText,
    todayText,
    verdictText,
    type Change,
    type NameDecision,
    type Preview,
    type Proposal,
    type Recorder,
    type Recorders,
    type SystemDecision,
    type Unknown,
  } from './recorders';
  import { admin, can, why } from './session.svelte';
  import TypedConfirm from './TypedConfirm.svelte';
  import { readWaiting, waiting } from './waiting.svelte';

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

  /** Each recorder's preview, once asked for, and why one could not be made. */
  let previews = $state<Record<number, Preview>>({});
  let previewRefusals = $state<Record<number, string>>({});
  let previewing = $state<number | null>(null);

  /** The system each unknown name is to be the same as, the name whose
   * discard waits for its name typed, and a decision's refusal, by name. */
  let sameAs = $state<Record<string, string>>({});
  let discarding = $state<string | null>(null);
  let systemRefusals = $state<Record<string, string>>({});
  /** A proposal's refusal, by talkgroup and name. */
  let nameRefusals = $state<Record<string, string>>({});
  const proposalKey = (p: Proposal) => `${p.talkgroup_id}/${p.proposed}`;

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

  async function preview(r: Recorder) {
    delete previewRefusals[r.id];
    previewing = r.id;
    try {
      previews[r.id] = await admin<Preview>('GET', `/api/admin/recorders/${r.id}/preview`);
    } catch (error) {
      previewRefusals[r.id] = why(error);
    } finally {
      previewing = null;
    }
  }

  async function decideSystem(u: Unknown, decision: SystemDecision, body: { to?: string; confirm?: string } = {}) {
    delete systemRefusals[u.system];
    try {
      const done = await admin<{ discarded: number } | undefined>('POST', `/api/admin/staging/${decision}`, {
        system: u.system,
        ...body,
      });
      discarding = null;
      tell(systemDecidedText(u.system, decision, { to: body.to, discarded: done?.discarded }));
      await readWaiting();
    } catch (error) {
      systemRefusals[u.system] = why(error);
    }
  }

  async function decideName(p: Proposal, decision: NameDecision) {
    delete nameRefusals[proposalKey(p)];
    try {
      await admin('POST', `/api/admin/names/${decision}`, { talkgroup_id: p.talkgroup_id, name: p.proposed });
      tell(nameDecidedText(p, decision));
      await readWaiting();
    } catch (error) {
      nameRefusals[proposalKey(p)] = why(error);
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
          {#if r.clock}<p class="problem">{clockSentence(r.clock)}</p>{/if}
          {#if removing === r.id}
            <p>Remove {r.name}? Its files stay in its folder and are no longer taken; its calls stay in the archive.</p>
            <div class="row">
              <button type="button" class="primary" onclick={() => apply(r, 'remove')}>Remove {r.name}</button>
              <button type="button" onclick={() => (removing = null)}>Keep {r.name}</button>
            </div>
          {:else}
            <div class="row">
              <button type="button" aria-label={`Preview ${r.name}`} disabled={previewing === r.id} onclick={() => preview(r)}>
                Preview
              </button>
              {#each can('recorders') ? changes(r) : [] as change (change)}
                <button type="button" aria-label={`${CHANGE_LABEL[change]} ${r.name}`}
                  onclick={() => (change === 'remove' ? (removing = r.id) : apply(r, change))}>
                  {CHANGE_LABEL[change]}
                </button>
              {/each}
            </div>
          {/if}
          {#if refusals[r.id]}<p class="problem" role="alert">Not changed: {refusals[r.id]}</p>{/if}
          {#if previewRefusals[r.id]}<p class="problem" role="alert">No preview: {previewRefusals[r.id]}</p>{/if}
          {#if previews[r.id]}
            {@const p = previews[r.id]}
            <section class="preview" aria-label={`Preview of ${r.name}`}>
              <p>{previewText(p)} The preview changed nothing.</p>
              {#if p.sample.length}
                <!-- Wider than a phone, it scrolls on its own; a keyboard scrolls it once it has the focus. -->
                <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
                <div class="scroll" role="region" aria-label={`The files read from ${r.name}`} tabindex="0">
                  <table>
                    <thead>
                      <tr><th>Time</th><th>System / site</th><th>Talkgroup</th><th>Radio</th><th>Length</th><th>Filing</th><th>File</th></tr>
                    </thead>
                    <tbody>
                      {#each p.sample as f (f.rel_path)}
                        <tr>
                          <td>{f.started_at ? callTime(f.started_at, now, siteZone()) : '—'}</td>
                          <td>{f.system ?? '—'}{f.site ? ` / ${f.site}` : ''}</td>
                          <td>{f.talkgroup === null ? '—' : `${f.talkgroup} ${f.talkgroup_name ?? ''}`}</td>
                          <td>{source(f) || '—'}</td>
                          <td>{seconds(f.duration_ms) || '—'}</td>
                          <td>{verdictText(f.verdict)}</td>
                          <td class="path">{f.rel_path}</td>
                        </tr>
                      {/each}
                    </tbody>
                  </table>
                </div>
              {/if}
              <div class="row"><button type="button" onclick={() => delete previews[r.id]}>Close the preview</button></div>
            </section>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}

  {#if can('recorders')}
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
  {/if}

  <h2>Waiting for you</h2>
  {#if waiting.error}<p class="problem" role="alert">What waits could not be read: {waiting.error}</p>{/if}
  {#if waiting.data === null}
    <p class="empty">Reading…</p>
  {:else if waiting.data.waiting === 0}
    <p class="empty">Nothing waits: every system name in the files is known, and no recorder sent another name for a talkgroup.</p>
  {:else}
    {@const systems = waiting.data.systems}
    {#if waiting.data.unknown.length}
      <h3>Unknown system names</h3>
      <p>Their files wait in the archive's staging folder, off the recorders' shares, until you decide.</p>
      <ul class="recorders">
        {#each waiting.data.unknown as u, i (u.system)}
          <li aria-labelledby={`unknown-${i}`}>
            <h4 id={`unknown-${i}`}>{u.system}</h4>
            <dl class="facts">
              <dt>Files</dt><dd>{u.files.toLocaleString('en-US')}</dd>
              <dt>From</dt><dd>{u.recorders.join(', ')}</dd>
              <dt>First / last seen</dt><dd>{seenText(u.first_seen, u.last_seen, now, siteZone())}</dd>
              <dt>Some of its talkgroups</dt><dd>{u.talkgroups.join(', ')}</dd>
            </dl>
            {#if discarding === u.system}
              <p>Discard {u.system}? Its {counted(u.files, 'staged file')} are deleted, and it stays unknown.</p>
              <TypedConfirm name={u.system} action="Discard its files" start={(confirm) => decideSystem(u, 'discard', { confirm })} />
              <div class="row"><button type="button" onclick={() => (discarding = null)}>Keep its files</button></div>
            {:else if can('names')}
              <div class="row">
                <button type="button" aria-label={`New system ${u.system}`} onclick={() => decideSystem(u, 'new-system')}>New system</button>
                <label class="same">Same as
                  <select aria-label={`Same as, for ${u.system}`} bind:value={sameAs[u.system]}>
                    <option value={undefined}>Pick a system</option>
                    {#each systems as system (system)}<option value={system}>{system}</option>{/each}
                  </select>
                </label>
                <button type="button" aria-label={`Map ${u.system}`} disabled={!sameAs[u.system]}
                  onclick={() => decideSystem(u, 'map', { to: sameAs[u.system] })}>Map</button>
                <button type="button" aria-label={`Discard… ${u.system}`} onclick={() => (discarding = u.system)}>Discard…</button>
              </div>
            {/if}
            {#if systemRefusals[u.system]}<p class="problem" role="alert">Not decided: {systemRefusals[u.system]}</p>{/if}
          </li>
        {/each}
      </ul>
    {/if}
    {#if waiting.data.proposals.length}
      <h3>Talkgroup names to approve</h3>
      <p>
        Calls keep filing under the current name until you decide. An approved name renames the talkgroup, and its files
        wait for a correction on the Data area; a dismissed one is ignored from then on.
      </p>
      <ul class="recorders">
        {#each waiting.data.proposals as p, i (proposalKey(p))}
          <li aria-labelledby={`proposal-${i}`}>
            <h4 id={`proposal-${i}`}>{p.system} {p.number}: {p.current} → {p.proposed}</h4>
            <dl class="facts">
              <dt>From</dt><dd>{p.recorders.join(', ')}</dd>
              <dt>Calls</dt><dd>{p.calls.toLocaleString('en-US')}</dd>
              <dt>First / last seen</dt><dd>{seenText(p.first_seen, p.last_seen, now, siteZone())}</dd>
            </dl>
            {#if can('names')}
              <div class="row">
                <button type="button" aria-label={`Approve ${p.proposed}`} onclick={() => decideName(p, 'approve')}>Approve</button>
                <button type="button" aria-label={`Dismiss ${p.proposed}`} onclick={() => decideName(p, 'dismiss')}>Dismiss</button>
              </div>
            {/if}
            {#if nameRefusals[proposalKey(p)]}<p class="problem" role="alert">Not decided: {nameRefusals[proposalKey(p)]}</p>{/if}
          </li>
        {/each}
      </ul>
    {/if}
  {/if}
</section>

<style>
  .recorders { list-style: none; margin: 0; padding: 0; display: grid; grid-template-columns: minmax(0, 1fr); gap: 0.75rem; }
  .recorders li { border: 1px solid var(--line); border-radius: 6px; padding: 0.7rem 0.9rem; background: var(--surface); overflow-wrap: anywhere; }
  .recorders h2, .recorders h4 { margin: 0 0 0.4rem; font-size: 1.05rem; }
  .recorders h2 span { font-size: 0.85rem; font-weight: 400; }
  .facts { margin: 0.2rem 0 0.6rem; }
  .grid { display: grid; grid-template-columns: minmax(0, 1fr); gap: 0.4rem; max-width: 36rem; }
  .grid > label { font-weight: 600; }
  fieldset { border: 1px solid var(--line); border-radius: 4px; padding: 0.5rem 0.7rem; display: grid; gap: 0.4rem; min-width: 0; }
  legend { font-weight: 600; }
  .preview { margin-top: 0.6rem; }
  .scroll { overflow-x: auto; }
  .preview table { border-collapse: collapse; font-size: 0.9rem; }
  .preview th, .preview td { text-align: left; padding: 0.25rem 0.5rem; border-bottom: 1px solid var(--line); vertical-align: top; }
  /* The table keeps its words whole and scrolls instead; a file's path may break anywhere. */
  .preview td, .preview th { overflow-wrap: normal; }
  .preview .path { overflow-wrap: anywhere; min-width: 12rem; }
  .same { display: inline-flex; gap: 0.4rem; align-items: center; }
</style>
