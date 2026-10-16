<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { ApiError } from '../api';
  import { counted } from '../format';
  import { tell } from '../notice.svelte';
  import { admin, why } from './session.svelte';
  import {
    correctText, mergeText, purgeText, readOnlyText, takenBy,
    type CorrectPreview, type MergePreview, type PurgePreview, type Started, type Uncorrected,
  } from './data';
  import type { AdminTalkgroup } from './transcription';
  import TypedConfirm from './TypedConfirm.svelte';

  let talkgroups = $state<AdminTalkgroup[]>([]);
  let uncorrected = $state<Uncorrected[] | null>(null);
  /** Why each list could not be read, in the server's words. */
  let problem = $state<string | null>(null);
  let filesProblem = $state<string | null>(null);
  const byId = (id: number | null) => talkgroups.find((t) => t.id === id);
  const label = (t: AdminTalkgroup) => `${t.name} · ${t.system} ${t.number}`;

  let reads = 0;
  /** Reads both lists, each on its own: one that fails does not empty the other. */
  async function read() {
    const mine = ++reads;
    const [all, files] = await Promise.allSettled([
      admin<AdminTalkgroup[]>('GET', '/api/admin/talkgroups'),
      admin<Uncorrected[]>('GET', '/api/admin/data/uncorrected'),
    ]);
    // An answer to a read that a later one has replaced is dropped.
    if (mine !== reads) return;
    if (all.status === 'fulfilled') {
      talkgroups = all.value;
      problem = null;
    } else problem = why(all.reason);
    if (files.status === 'fulfilled') {
      uncorrected = files.value;
      filesProblem = null;
    } else filesProblem = why(files.reason);
  }
  onMount(read);

  /** Another admin may have renamed, merged or purged a talkgroup since the lists were read: after a refusal
   * or a start they are read again. The fields that hang on them settle before the caller goes on. */
  async function reread() {
    await read();
    await tick();
  }

  // Rename.
  const renaming = $state<{ id: number | null; name: string }>({ id: null, name: '' });
  let renamed = $state<string | null>(null);
  let renameRefusal = $state<string | null>(null);
  let renameBusy = $state(false);
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
    if (!subject || renameBusy) return;
    renamed = renameRefusal = null;
    taken = null;
    renameBusy = true;
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
      await read();
      if (error instanceof ApiError && error.status === 409) taken = takenBy(talkgroups, subject, renaming.name) ?? null;
    } finally {
      renameBusy = false;
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
      if (!current()) return;
      const words = why(error);
      await reread();
      // The list may no longer offer the talkgroup counted, which empties its select and so clears a
      // refusal: it is shown after, unless the person has chosen another since.
      if (merge.from === from && (merge.into === into || merge.into === null)) mergeRefusal = words;
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
      tell(`Job #${job.id} merges ${counted(job.count, 'call')}; follow it in Jobs.`);
      mergePreview = null;
      // The confirm box goes; focus must not go with it.
      document.getElementById('merge-from')?.focus();
      await read();
    } catch (error) {
      const words = why(error);
      await reread();
      mergeRefusal = words;
    }
  }

  // Correct: a renamed talkgroup's files move to its name's folder.
  let correctHeading = $state<HTMLElement>();
  let filesTable = $state<HTMLTableElement>();
  let correcting: number | null = null;
  let correctPreview = $state<CorrectPreview | null>(null);
  let correctRefusal = $state<string | null>(null);

  async function countCorrect(id: number) {
    correctPreview = correctRefusal = null;
    correcting = id;
    // An answer, or a refusal, for another row's Correct clicked since is dropped.
    const current = () => correcting === id;
    try {
      const found = await admin<CorrectPreview>('GET', `/api/admin/talkgroups/${id}/correct`);
      if (current()) correctPreview = found;
    } catch (error) {
      if (current()) {
        correctRefusal = why(error);
        await read();
      }
    }
  }

  async function startCorrect(confirm: string) {
    if (!correctPreview) return;
    correctRefusal = null;
    try {
      const job = await admin<Started>('POST', `/api/admin/talkgroups/${correctPreview.talkgroup.id}/correct`, { confirm });
      tell(`Job #${job.id} corrects ${counted(job.count, 'file')}; follow it in Jobs.`);
      correctPreview = null;
      (filesTable?.querySelector('button') ?? correctHeading)?.focus();
      await read();
    } catch (error) {
      correctRefusal = why(error);
      await read();
    }
  }

  // Purge: a talkgroup's calls are deleted, their files kept in the holding folder.
  let purging = $state<number | null>(null);
  let purgePreview = $state<PurgePreview | null>(null);
  let purgeRefusal = $state<string | null>(null);
  $effect(() => {
    void purging;
    purgePreview = purgeRefusal = null;
  });

  async function countPurge(event: SubmitEvent) {
    event.preventDefault();
    const id = purging;
    if (id === null) return;
    purgeRefusal = null;
    // An answer, or a refusal, for a talkgroup that has changed since is dropped.
    const current = () => purging === id;
    try {
      const found = await admin<PurgePreview>('GET', `/api/admin/talkgroups/${id}/purge`);
      if (current()) purgePreview = found;
    } catch (error) {
      if (current()) {
        purgeRefusal = why(error);
        await read();
      }
    }
  }

  async function startPurge(confirm: string) {
    if (!purgePreview) return;
    purgeRefusal = null;
    try {
      const job = await admin<Started>('POST', `/api/admin/talkgroups/${purgePreview.talkgroup.id}/purge`, { confirm });
      tell(`Job #${job.id} purges ${counted(job.count, 'call')}; follow it in Jobs.`);
      purgePreview = null;
      document.getElementById('purge-talkgroup')?.focus();
      await read();
    } catch (error) {
      purgeRefusal = why(error);
      await read();
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
    <div class="row"><button type="submit" disabled={renaming.id === null || renameBusy}>Rename</button></div>
    {#if renamed}<p role="status">{renamed}</p>{/if}
    {#if renameRefusal}
      <p class="problem" role="alert">Not renamed: {renameRefusal}</p>
      {#if taken}<div class="row"><button type="button" onclick={mergeIntoTaken}>Merge into it</button></div>{/if}
    {/if}
  </form>

  <h2 tabindex="-1" bind:this={correctHeading}>Files not yet corrected</h2>
  <p>
    Talkgroups renamed since their files were named. A correction moves each call's file to the folder of the
    talkgroup's name; files in read-only locations stay where they are.
  </p>
  {#if filesProblem}<p class="problem" role="alert">The files not yet corrected could not be read: {filesProblem}</p>{/if}
  {#if uncorrected === null}
    {#if !filesProblem}<p class="empty">Reading…</p>{/if}
  {:else if uncorrected.length === 0}
    <p class="empty">Every talkgroup's files carry its name.</p>
  {:else}
    <table bind:this={filesTable}>
      <caption class="visually-hidden">Talkgroups whose files are not yet corrected</caption>
      <thead><tr><th scope="col">Talkgroup</th><th scope="col">System and number</th><th scope="col">Its files' name</th><th scope="col" class="num">Calls</th><th scope="col"><span class="visually-hidden">Correct</span></th></tr></thead>
      <tbody>
        {#each uncorrected as t (t.id)}
          <tr>
            <td class="long">{t.name}</td>
            <td>{t.system} {t.number}</td>
            <td class="long">{t.files_name ?? '—'}</td>
            <td class="num">{t.calls.toLocaleString('en-US')}</td>
            <td><button type="button" aria-label={`Correct the files of ${t.name} (${t.system} ${t.number})`} onclick={() => countCorrect(t.id)}>Correct</button></td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
  {#if correctPreview}
    <p role="status">{correctText(correctPreview)}</p>
    {#key correctPreview}<TypedConfirm name={correctPreview.talkgroup.name} action="Correct the files" start={startCorrect} />{/key}
  {/if}
  {#if correctRefusal}<p class="problem" role="alert">Not corrected: {correctRefusal}</p>{/if}

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
    <p role="status">{mergeText(mergePreview)}</p>
    {#key mergePreview}<TypedConfirm name={mergePreview.from.name} action="Merge" start={startMerge} />{/key}
  {/if}
  {#if mergeRefusal}<p class="problem" role="alert">Not merged: {mergeRefusal}</p>{/if}

  <h2>Purge</h2>
  <p>
    A purge deletes a talkgroup's calls; their files go to the holding folder, the setting
    <code>purge.holding_path</code>, to be deleted by hand once no one needs them. The holding folder must be
    outside the archive's locations and the ingest sources, and hold an empty file named
    <code>.scannerpulse-holding</code>, which shows it is the real folder, mounted: without it the job stops.
  </p>
  <form class="grid" onsubmit={countPurge}>
    <label for="purge-talkgroup">Talkgroup to purge</label>
    <select id="purge-talkgroup" bind:value={purging}>
      <option value={null}>Pick one</option>
      {#each talkgroups as t (t.id)}<option value={t.id}>{label(t)}</option>{/each}
    </select>
    <div class="row"><button type="submit" disabled={purging === null}>Count the calls to delete</button></div>
  </form>
  {#if purgePreview}
    {#if purgePreview.holding_path === null}
      <p class="problem" role="alert">Set <code>purge.holding_path</code> in Settings first: the purged calls' files are kept there.</p>
    {:else if purgePreview.read_only > 0}
      <p class="problem" role="alert">{readOnlyText(purgePreview.read_only)}</p>
    {:else}
      <p role="status" class="wrap">{purgeText(purgePreview)}</p>
      {#key purgePreview}<TypedConfirm name={purgePreview.talkgroup.name} action="Purge" start={startPurge} />{/key}
    {/if}
  {/if}
  {#if purgeRefusal}<p class="problem" role="alert">Not purged: {purgeRefusal}</p>{/if}
</section>

<style>
  .grid { display: grid; gap: 0.4rem; max-width: 36rem; }
  .grid label { font-weight: 600; }
  select { max-width: 100%; }
  /* The holding folder's path is one long word. */
  .wrap { overflow-wrap: anywhere; }
</style>
