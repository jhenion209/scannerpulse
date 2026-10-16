<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { ApiError } from '../api';
  import { tell } from '../notice.svelte';
  import { admin, why } from './session.svelte';
  import { takenBy, written, type MergePreview, type Started } from './data';
  import type { AdminTalkgroup } from './transcription';
  import TypedConfirm from './TypedConfirm.svelte';

  let talkgroups = $state<AdminTalkgroup[]>([]);
  let problem = $state<string | null>(null);
  const byId = (id: number | null) => talkgroups.find((t) => t.id === id);
  const label = (t: AdminTalkgroup) => `${t.name} · ${t.system} ${t.number}`;

  async function read() {
    try {
      talkgroups = await admin<AdminTalkgroup[]>('GET', '/api/admin/talkgroups');
      problem = null;
    } catch (error) {
      problem = why(error);
    }
  }
  onMount(read);

  // Rename.
  const renaming = $state<{ id: number | null; name: string }>({ id: null, name: '' });
  let renamed = $state<string | null>(null);
  let renameRefusal = $state<string | null>(null);
  /** The talkgroup that has the name already, after a refused rename. */
  let taken = $state<AdminTalkgroup | null>(null);

  function pickRename() {
    renaming.name = byId(renaming.id)?.name ?? '';
    renamed = renameRefusal = null;
    taken = null;
  }

  async function rename(event: SubmitEvent) {
    event.preventDefault();
    const subject = byId(renaming.id);
    if (!subject) return;
    renamed = renameRefusal = null;
    taken = null;
    try {
      const answer = await admin<{ name: string; files_correct?: boolean; unchanged?: boolean }>(
        'POST', `/api/admin/talkgroups/${subject.id}/rename`, { name: renaming.name });
      renamed = answer.unchanged
        ? 'The name is unchanged.'
        : answer.files_correct
          ? `Renamed to ${answer.name}.`
          : `Renamed to ${answer.name}. Its calls' files keep the old name until they are corrected.`;
      await read();
    } catch (error) {
      renameRefusal = why(error);
      if (error instanceof ApiError && error.status === 409) {
        await read();
        taken = takenBy(talkgroups, subject, renaming.name) ?? null;
      }
    }
  }

  // Merge: one talkgroup's calls go to another of its system.
  const merge = $state<{ from: number | null; into: number | null }>({ from: null, into: null });
  const intoChoices = $derived(talkgroups.filter((t) => t.system === byId(merge.from)?.system && t.id !== merge.from));
  let mergePreview = $state<MergePreview | null>(null);
  let mergeRefusal = $state<string | null>(null);
  $effect(() => {
    void [merge.from, merge.into];
    mergePreview = null;
    mergeRefusal = null;
  });
  $effect(() => {
    if (merge.into !== null && !intoChoices.some((t) => t.id === merge.into)) merge.into = null;
  });

  async function countMerge() {
    const { from, into } = merge;
    if (from === null || into === null) return;
    mergeRefusal = null;
    // An answer, or a refusal, for fields that have changed since is dropped.
    const current = () => merge.from === from && merge.into === into;
    try {
      const found = await admin<MergePreview>('GET', `/api/admin/talkgroups/${from}/merge?into=${into}`);
      if (current()) mergePreview = found;
    } catch (error) {
      if (current()) mergeRefusal = why(error);
    }
  }

  async function mergeIntoTaken() {
    const subject = byId(renaming.id);
    if (!subject || !taken) return;
    merge.from = subject.id;
    merge.into = taken.id;
    document.getElementById('merge-from')?.focus();
    // After the effect that clears a stale count has run.
    await tick();
    await countMerge();
  }

  async function startMerge(confirm: string) {
    if (!mergePreview) return;
    mergeRefusal = null;
    try {
      const job = await admin<Started>('POST', `/api/admin/talkgroups/${mergePreview.from.id}/merge`, {
        into: mergePreview.into.id,
        confirm,
      });
      tell(`Job #${job.id} merges ${job.count.toLocaleString('en-US')} calls; follow it in Jobs.`);
      mergePreview = null;
    } catch (error) {
      mergeRefusal = why(error);
    }
  }
</script>

<section class="area" aria-labelledby="data-title">
  <h1 id="data-title">Data</h1>
  <p class="lead">
    Talkgroups' names and their files. A job that moves or deletes files is counted first, and starts only with
    the talkgroup's name typed exactly.
  </p>
  {#if problem}<p class="problem" role="alert">The talkgroups could not be read: {problem}</p>{/if}

  <h2>Rename</h2>
  <p>
    Calls show the new name at once; their files keep the old one until they are corrected. A talkgroup the
    recorder still sends takes the recorder's name back with its next call.
  </p>
  <form class="grid" onsubmit={rename}>
    <label for="rename-talkgroup">Talkgroup to rename</label>
    <select id="rename-talkgroup" bind:value={renaming.id} onchange={pickRename}>
      <option value={null}>Pick one</option>
      {#each talkgroups as t (t.id)}<option value={t.id}>{label(t)}</option>{/each}
    </select>
    <label for="rename-name">New name</label>
    <input id="rename-name" type="text" autocomplete="off" bind:value={renaming.name} />
    <div class="row"><button type="submit" disabled={renaming.id === null}>Rename</button></div>
    {#if renamed}<p role="status">{renamed}</p>{/if}
    {#if renameRefusal}
      <p class="problem" role="alert">Not renamed: {renameRefusal}</p>
      {#if taken}<div class="row"><button type="button" onclick={mergeIntoTaken}>Merge into it</button></div>{/if}
    {/if}
  </form>

  <h2>Merge</h2>
  <p>The calls of one talkgroup go to another of the same system, and their files are corrected; the first talkgroup goes.</p>
  <form class="grid" onsubmit={(event) => { event.preventDefault(); void countMerge(); }}>
    <label for="merge-from">Talkgroup that goes</label>
    <select id="merge-from" bind:value={merge.from}>
      <option value={null}>Pick one</option>
      {#each talkgroups as t (t.id)}<option value={t.id}>{label(t)}</option>{/each}
    </select>
    <label for="merge-into">Talkgroup that stays</label>
    <select id="merge-into" bind:value={merge.into}>
      <option value={null}>Pick one</option>
      {#each intoChoices as t (t.id)}<option value={t.id}>{label(t)}</option>{/each}
    </select>
    <div class="row"><button type="submit" disabled={merge.from === null || merge.into === null}>Count the calls to move</button></div>
  </form>
  {#if mergePreview}
    <p role="status">{mergePreview.calls.toLocaleString('en-US')} calls of {written(mergePreview.from)} go to {written(mergePreview.into)}; {mergePreview.from.name} goes.</p>
    {#key mergePreview}<TypedConfirm name={mergePreview.from.name} action="Merge" start={startMerge} />{/key}
  {/if}
  {#if mergeRefusal}<p class="problem" role="alert">Not merged: {mergeRefusal}</p>{/if}
</section>

<style>
  .grid { display: grid; gap: 0.4rem; max-width: 36rem; }
  .grid label { font-weight: 600; }
  select { max-width: 100%; }
</style>
