<script lang="ts">
  import type { FilterChoices } from './api';
  import { matchGroups, toggle } from './talkgroups';

  let { chosen = $bindable(), choices }: { chosen: number[]; choices: FilterChoices } = $props();

  let search = $state('');
  const groups = $derived(matchGroups(choices.talkgroup_groups, search));
</script>

<fieldset class="field">
  <legend>Talkgroups</legend>
  <input type="search" placeholder="Find a talkgroup" aria-label="Find a talkgroup" bind:value={search} />
  {#if chosen.length}
    <p class="chosen">
      {chosen.length} chosen
      <button type="button" class="link" onclick={() => (chosen = [])}>Clear</button>
    </p>
  {/if}
  <div class="groups">
    {#each groups as group (group.system_id)}
      <div class="group" role="group" aria-label={group.system}>
        <div class="system">{group.system}</div>
        {#each group.talkgroups as talkgroup (talkgroup.id)}
          <label class="talkgroup">
            <input type="checkbox" checked={chosen.includes(talkgroup.id)} onchange={() => (chosen = toggle(chosen, talkgroup.id))} />
            {talkgroup.name}
            <span class="number">{talkgroup.number}</span>
          </label>
        {/each}
      </div>
    {/each}
  </div>
</fieldset>

<style>
  .field { display: flex; flex-direction: column; gap: 0.3rem; border: 0; margin: 0; padding: 0; min-width: 0; }
  legend { font-size: 0.8rem; font-weight: 600; text-transform: uppercase; color: var(--muted); padding: 0; }
  input[type='search'] {
    background: var(--bg); border: 1px solid var(--field-border); border-radius: 4px; padding: 0.3rem 0.4rem; min-height: 2.25rem; width: 100%;
  }
  .groups { max-height: min(18rem, 30dvh); overflow: auto; border: 1px solid var(--field-border); border-radius: 4px; background: var(--bg); }
  .system { position: sticky; top: 0; background: var(--surface); font-weight: 600; padding: 0.2rem 0.5rem; }
  .talkgroup { display: flex; gap: 0.4rem; align-items: center; padding: 0.2rem 0.5rem; min-height: 2rem; }
  .number { margin-left: auto; color: var(--muted); font-size: 0.85rem; }
  .chosen { margin: 0; font-size: 0.9rem; color: var(--muted); }
  .link { background: none; border: 0; padding: 0; color: var(--accent); text-decoration: underline; cursor: pointer; text-align: left; }
</style>
