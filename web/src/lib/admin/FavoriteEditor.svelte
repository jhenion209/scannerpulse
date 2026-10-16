<script lang="ts">
  import type { TalkgroupGroup } from '../api';
  import { matchGroups, toggle } from '../talkgroups';
  import { why } from './session.svelte';

  let { id, name = null, chosen = [], groups, taken = [], onsave, oncancel }: {
    /** Makes the fields' ids unique on the page. */
    id: string;
    /** The list's name; null for a new list, whose name is typed. */
    name?: string | null;
    chosen?: number[];
    groups: TalkgroupGroup[];
    /** The names of the lists that exist: a new list may not take one (the server would replace that list). */
    taken?: string[];
    /** Saves; throws the server's refusal, which the editor shows. */
    onsave: (name: string, talkgroups: number[]) => Promise<void>;
    oncancel?: () => void;
  } = $props();

  // svelte-ignore state_referenced_locally
  let picked = $state([...chosen]);
  let typed = $state('');
  let search = $state('');
  let refusal = $state<string | null>(null);
  let busy = $state(false);
  const shown = $derived(matchGroups(groups, search));

  async function save(event: SubmitEvent) {
    event.preventDefault();
    const listName = name ?? typed.trim();
    refusal = listName ? null : 'Name the list first.';
    if (name === null && taken.includes(listName)) refusal = `A list named ${listName} exists: use its Edit button.`;
    if (refusal || busy) return;
    busy = true;
    try {
      await onsave(listName, picked);
      if (name === null) {
        typed = '';
        picked = [];
      }
    } catch (error) {
      refusal = why(error);
    } finally {
      busy = false;
    }
  }
</script>

<form class="editor" onsubmit={save}>
  {#if name === null}
    <label for={`${id}-name`}>Name</label>
    <input id={`${id}-name`} bind:value={typed} />
  {/if}
  <fieldset>
    <legend>Talkgroups ({picked.length} chosen)</legend>
    <input type="search" aria-label="Find a talkgroup" placeholder="Find a talkgroup" bind:value={search}
      onkeydown={(event) => { if (event.key === 'Enter') event.preventDefault(); }} />
    <div class="groups">
      {#each shown as group (group.system_id)}
        <div role="group" aria-label={group.system}>
          <div class="system">{group.system}</div>
          {#each group.talkgroups as talkgroup (talkgroup.id)}
            <label class="talkgroup">
              <input type="checkbox" checked={picked.includes(talkgroup.id)} onchange={() => (picked = toggle(picked, talkgroup.id))} />
              {talkgroup.name} <span class="number">{talkgroup.number}</span>
            </label>
          {/each}
        </div>
      {/each}
    </div>
  </fieldset>
  <div class="row">
    <button type="submit" class="primary" disabled={busy}>{name === null ? 'Add the list' : `Save ${name}`}</button>
    {#if oncancel}<button type="button" onclick={oncancel}>Cancel</button>{/if}
  </div>
  {#if refusal}<p class="problem" role="alert">Not saved: {refusal}</p>{/if}
</form>

<style>
  .editor { display: grid; gap: 0.4rem; max-width: 32rem; margin: 0.5rem 0; }
  label { font-weight: 600; }
  fieldset { border: 1px solid var(--line); border-radius: 4px; padding: 0.5rem 0.7rem; display: grid; gap: 0.4rem; min-width: 0; }
  legend { font-weight: 600; }
  .groups { max-height: 18rem; overflow: auto; }
  .system { margin-top: 0.4rem; color: var(--muted); font-weight: 600; }
  .talkgroup { display: flex; gap: 0.4rem; align-items: center; font-weight: 400; padding: 0.1rem 0; }
  .number { color: var(--muted); }
</style>
