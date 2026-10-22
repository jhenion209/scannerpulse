<script lang="ts">
  import { onMount } from 'svelte';
  import { zoned } from '../format';
  import { tell } from '../notice.svelte';
  import { siteZone } from '../status.svelte';
  import { STATUS_EVERY_MS } from './health';
  import { admin, why } from './session.svelte';
  import {
    CONFIRM,
    callsText,
    freeText,
    moveAction,
    moveAsk,
    previewText,
    purgeAction,
    purgeAsk,
    purgeText,
    seenBy,
    type Location,
    type MovePreview,
    type PurgePreview,
    type Storage,
  } from './storage';
  import TypedConfirm from './TypedConfirm.svelte';

  let data = $state<Storage | null>(null);
  let problem = $state<string | null>(null);
  let now = $state(new Date());
  /** The form open below the buttons. */
  let open = $state<'add' | 'move' | 'purge' | null>(null);
  /** The location whose removal waits for a second click, and a refusal by location. */
  let removing = $state<number | null>(null);
  let refusals = $state<Record<number, string>>({});

  let name = $state('');
  let folder = $state('');
  let adding = $state(false);
  let addRefusal = $state<string | null>(null);

  let from = $state('');
  let until = $state('');
  let to = $state('');
  let counting = $state(false);
  let counted = $state<MovePreview | null>(null);
  let moveRefusal = $state<string | null>(null);

  let before = $state('');
  let purgeCounting = $state(false);
  let purgeCounted = $state<PurgePreview | null>(null);
  let purgeRefusal = $state<string | null>(null);

  async function read() {
    try {
      data = await admin<Storage>('GET', '/api/admin/storage', undefined, { poll: true });
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

  const label = (l: Location) => (l.primary ? 'new calls are filed here' : l.mode === 'readonly' ? 'read-only: its files are never changed' : null);
  const destinations = $derived((data?.locations ?? []).filter((l) => l.mode === 'managed'));

  function show(form: 'add' | 'move' | 'purge') {
    open = open === form ? null : form;
    addRefusal = null;
    moveRefusal = null;
    counted = null;
    purgeRefusal = null;
    purgeCounted = null;
  }

  async function add(event: SubmitEvent) {
    event.preventDefault();
    adding = true;
    addRefusal = null;
    try {
      await admin('POST', '/api/admin/storage', { name, folder });
      tell(`${name} is added; each ScannerPulse machine checks it within a minute.`);
      name = '';
      folder = '';
      open = null;
      await read();
    } catch (error) {
      addRefusal = why(error);
    } finally {
      adding = false;
    }
  }

  async function remove(l: Location) {
    delete refusals[l.id];
    try {
      await admin('DELETE', `/api/admin/storage/${l.id}`);
      removing = null;
      tell(`${l.name} is removed; its folder is left as it is.`);
      await read();
    } catch (error) {
      refusals[l.id] = why(error);
    }
  }

  /** Counts the move; an answer whose fields changed while it was asked for is dropped. */
  async function count(event: SubmitEvent) {
    event.preventDefault();
    const asked = { to, from, until };
    counting = true;
    moveRefusal = null;
    counted = null;
    try {
      const query = new URLSearchParams(asked);
      const answer = await admin<MovePreview>('GET', `/api/admin/storage/move?${query}`);
      if (asked.to === to && asked.from === from && asked.until === until) counted = answer;
    } catch (error) {
      moveRefusal = why(error);
    } finally {
      counting = false;
    }
  }

  /** Counts the purge; an answer for a day changed meanwhile is dropped. */
  async function countPurge(event: SubmitEvent) {
    event.preventDefault();
    const asked = before;
    purgeCounting = true;
    purgeRefusal = null;
    purgeCounted = null;
    try {
      const answer = await admin<PurgePreview>('GET', `/api/admin/storage/purge?${new URLSearchParams({ before: asked })}`);
      if (asked === before) purgeCounted = answer;
    } catch (error) {
      purgeRefusal = why(error);
    } finally {
      purgeCounting = false;
    }
  }

  /** Starts the purge that was counted, its day typed. */
  async function startPurge(p: PurgePreview, confirm: string) {
    try {
      const done = await admin<{ job: number; calls: number }>('POST', '/api/admin/storage/purge', { before: p.before, confirm });
      tell(`Job ${done.job} deletes ${done.calls.toLocaleString('en-US')} calls before ${p.before}; the Jobs area follows it.`);
      purgeCounted = null;
      open = null;
    } catch (error) {
      purgeRefusal = why(error);
    }
  }

  /** Starts what was counted, never the fields as they are now. */
  async function start(p: MovePreview, confirm: string) {
    try {
      const done = await admin<{ job: number; calls: number }>('POST', '/api/admin/storage/move', {
        to: p.to,
        from: p.from_day,
        until: p.to_day,
        confirm,
      });
      tell(`Job ${done.job} moves ${done.calls.toLocaleString('en-US')} calls into ${p.to}; the Jobs area follows it.`);
      counted = null;
      open = null;
    } catch (error) {
      moveRefusal = why(error);
    }
  }
</script>

<section class="area" aria-labelledby="storage-title">
  <h1 id="storage-title">Storage</h1>
  <p class="lead">
    The main archive, where new calls are filed, and the storage paths beside it that hold calls moved out of it.
    Each is mounted on every ScannerPulse machine; listeners find a moved call where it is.
  </p>
  {#if problem}<p class="problem" role="alert">The locations could not be read: {problem}</p>{/if}
  {#if data === null}
    <p class="empty">Reading…</p>
  {:else}
    <ul class="locations">
      {#each data.locations as l (l.id)}
        {@const free = freeText(l)}
        {@const seen = seenBy(l, data.hosts, now)}
        <li aria-labelledby={`location-${l.id}`}>
          <h2 id={`location-${l.id}`}>{l.name} {#if label(l)}<span>{label(l)}</span>{/if}</h2>
          <dl class="facts">
            <dt>Folder</dt><dd>{l.root}</dd>
            <dt>Calls</dt><dd>{callsText(l, siteZone())}</dd>
            {#if free}<dt>Space</dt><dd>{free}</dd>{/if}
            <dt>Seen by</dt>
            <dd>{seen.filter((s) => s.ok).map((s) => s.text).join(', ') || '—'}</dd>
          </dl>
          {#each seen.filter((s) => !s.ok) as trouble (trouble.host)}<p class="problem">{trouble.text}</p>{/each}
          {#if !l.primary && l.calls === 0}
            {#if removing === l.id}
              <p>Remove {l.name}? It holds no call; its folder is left as it is.</p>
              <div class="row">
                <button type="button" class="primary" onclick={() => remove(l)}>Remove {l.name}</button>
                <button type="button" onclick={() => (removing = null)}>Keep {l.name}</button>
              </div>
            {:else}
              <div class="row"><button type="button" aria-label={`Remove… ${l.name}`} onclick={() => (removing = l.id)}>Remove…</button></div>
            {/if}
          {/if}
          {#if refusals[l.id]}<p class="problem" role="alert">Not removed: {refusals[l.id]}</p>{/if}
        </li>
      {/each}
    </ul>
    <p class="empty">Calls counted at {zoned(new Date(data.counted_at), siteZone()).time}; counted again every ten minutes.</p>
  {/if}

  <div class="row actions">
    <button type="button" aria-expanded={open === 'add'} onclick={() => show('add')}>Add a storage path…</button>
    <button type="button" aria-expanded={open === 'move'} onclick={() => show('move')}>Move a date range…</button>
    <!-- Red and apart, far from the others: it deletes for good. -->
    <button type="button" class="danger apart" aria-expanded={open === 'purge'} onclick={() => show('purge')}>Purge older than…</button>
  </div>

  {#if open === 'add'}
    <h2>Add a storage path</h2>
    <p>
      Mount the disk or share on every ScannerPulse machine first, at the same folder. It is checked on this machine and
      marked with a <code>.scannerpulse-location</code> file before it is saved; each machine then says whether it can use it.
    </p>
    <form class="grid" onsubmit={add}>
      <label for="location-name">Name</label>
      <input id="location-name" type="text" autocomplete="off" maxlength="40" required bind:value={name} />
      <label for="location-folder">Folder</label>
      <input id="location-folder" type="text" autocomplete="off" required placeholder="/srv/cold-2024" bind:value={folder} />
      <div class="row"><button type="submit" class="primary" disabled={adding}>Add the location</button></div>
      {#if addRefusal}<p class="problem" role="alert">Not added: {addRefusal}</p>{/if}
    </form>
  {/if}

  {#if open === 'move'}
    <h2>Move a date range</h2>
    <p>
      The calls of those days, wherever they are, move into the location you choose: each file is copied and checked,
      its call then points at the copy, and the old file is deleted. Listeners notice nothing.
    </p>
    <form class="grid" onsubmit={count}>
      <label for="move-from">From</label>
      <input id="move-from" type="date" required bind:value={from} oninput={() => (counted = null)} />
      <label for="move-until">To</label>
      <input id="move-until" type="date" required bind:value={until} oninput={() => (counted = null)} />
      <label for="move-to">Into</label>
      <select id="move-to" required bind:value={to} onchange={() => (counted = null)}>
        <option value="">Choose a location</option>
        {#each destinations as l (l.id)}<option value={l.name}>{l.name}</option>{/each}
      </select>
      <div class="row"><button type="submit" disabled={counting}>Count first</button></div>
    </form>
    {#if moveRefusal}<p class="problem" role="alert">Not moved: {moveRefusal}</p>{/if}
    {#if counted}
      {@const p = counted}
      <p>{previewText(p)}</p>
      {#if p.refused}
        <p class="problem" role="alert">It cannot start: {p.refused}</p>
      {:else}
        <TypedConfirm name={CONFIRM} action={moveAction(p)} ask={moveAsk(p)} start={(confirm) => start(p, confirm)} />
      {/if}
    {/if}
  {/if}

  {#if open === 'purge'}
    <h2>Purge older than a day</h2>
    <p>
      Every call before that day goes: its record with its transcripts and labels at once, and its file for good, deleted by
      the filer soon after. Nothing is kept. A read-only location's files stay: only their records go, and its import skips them.
    </p>
    <form class="grid" onsubmit={countPurge}>
      <label for="purge-before">Before</label>
      <input id="purge-before" type="date" required bind:value={before} oninput={() => (purgeCounted = null)} />
      <div class="row"><button type="submit" disabled={purgeCounting}>Count first</button></div>
    </form>
    {#if purgeRefusal}<p class="problem" role="alert">Not purged: {purgeRefusal}</p>{/if}
    {#if purgeCounted}
      {@const p = purgeCounted}
      <p>{purgeText(p)}</p>
      {#if p.refused}
        <p class="problem" role="alert">It cannot start: {p.refused}</p>
      {:else}
        <TypedConfirm name={p.before} action={purgeAction(p)} ask={purgeAsk(p)} danger start={(confirm) => startPurge(p, confirm)} />
      {/if}
    {/if}
  {/if}
</section>

<style>
  .locations { list-style: none; margin: 0; padding: 0; display: grid; grid-template-columns: minmax(0, 1fr); gap: 0.75rem; }
  .locations li { border: 1px solid var(--line); border-radius: 6px; padding: 0.7rem 0.9rem; background: var(--surface); overflow-wrap: anywhere; }
  .locations h2 { margin: 0 0 0.4rem; font-size: 1.05rem; }
  .locations h2 span { font-size: 0.85rem; font-weight: 400; color: var(--muted); }
  .actions { margin-top: 1rem; }
  .actions .apart { margin-left: auto; }
  .grid { display: grid; grid-template-columns: minmax(0, 1fr); gap: 0.4rem; max-width: 36rem; }
  .grid > label { font-weight: 600; }
</style>
