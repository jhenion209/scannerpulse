<script lang="ts">
  import type { Favorite, FilterChoices } from './api';
  import type { Filters } from './filters';
  import { matchGroups, toggle } from './talkgroups';

  let {
    filters = $bindable(),
    paused = $bindable(false),
    choices,
    favorites,
  }: { filters: Filters; paused?: boolean; choices: FilterChoices; favorites: Favorite[] } = $props();

  let search = $state('');
  const groups = $derived(matchGroups(choices.talkgroup_groups, search));
  const chosenNames = $derived(
    choices.talkgroup_groups
      .flatMap((g) => g.talkgroups)
      .filter((t) => filters.talkgroups.includes(t.id))
      .map((t) => t.name),
  );

  const set = (change: Partial<Filters>) => (filters = { ...filters, ...change });
  const number = (value: string) => (value === '' ? null : Number(value));
</script>

<div class="rail">
  <label class="field">
    <span>Favorite</span>
    <select
      value={filters.favorite ?? ''}
      onchange={(e) => set({ favorite: number(e.currentTarget.value) })}
    >
      <option value="">None</option>
      {#each favorites as favorite (favorite.id)}
        <option value={favorite.id}>{favorite.name}</option>
      {/each}
    </select>
  </label>

  <fieldset class="field">
    <legend>Talkgroups</legend>
    <input type="search" placeholder="Find a talkgroup" aria-label="Find a talkgroup" bind:value={search} />
    {#if chosenNames.length}
      <p class="chosen">
        {chosenNames.length} chosen
        <button type="button" class="link" onclick={() => set({ talkgroups: [] })}>Clear</button>
      </p>
    {/if}
    <div class="groups">
      {#each groups as group (group.system_id)}
        <div class="group" role="group" aria-label={group.system}>
          <div class="system">{group.system}</div>
          {#each group.talkgroups as talkgroup (talkgroup.id)}
            <label class="talkgroup">
              <input
                type="checkbox"
                checked={filters.talkgroups.includes(talkgroup.id)}
                onchange={() => set({ talkgroups: toggle(filters.talkgroups, talkgroup.id) })}
              />
              {talkgroup.name}
              <span class="number">{talkgroup.number}</span>
            </label>
          {/each}
        </div>
      {/each}
    </div>
  </fieldset>

  <label class="field">
    <span>System</span>
    <select value={filters.system ?? ''} onchange={(e) => set({ system: number(e.currentTarget.value) })}>
      <option value="">Every system</option>
      {#each choices.systems as system (system.id)}
        <option value={system.id}>{system.name}</option>
      {/each}
    </select>
  </label>

  <fieldset class="field">
    <legend>Dates</legend>
    <label class="check">
      <input
        type="checkbox"
        checked={filters.allDates}
        onchange={(e) => set({ allDates: e.currentTarget.checked })}
      />
      All dates
    </label>
    {#if !filters.allDates}
      <label class="sub">
        From
        <input
          type="datetime-local"
          value={filters.from ?? ''}
          onchange={(e) => set({ from: e.currentTarget.value || null })}
        />
      </label>
      <label class="sub">
        To
        <input
          type="datetime-local"
          value={filters.to ?? ''}
          onchange={(e) => set({ to: e.currentTarget.value || null })}
        />
      </label>
      {#if filters.from || filters.to}
        <button type="button" class="link" onclick={() => set({ from: null, to: null })}>
          Back to the last 24 hours
        </button>
      {:else}
        <p class="hint">The last 24 hours</p>
      {/if}
    {/if}
  </fieldset>

  <label class="check">
    <input type="checkbox" checked={filters.smart} onchange={(e) => set({ smart: e.currentTarget.checked })} />
    SmartSort
  </label>
  <label class="check">
    <input
      type="checkbox"
      checked={paused || filters.allDates}
      disabled={filters.allDates}
      onchange={(e) => (paused = e.currentTarget.checked)}
    />
    Pause updates
  </label>
</div>

<style>
  .rail { display: flex; flex-direction: column; gap: 0.9rem; padding: 0.9rem; }
  .field { display: flex; flex-direction: column; gap: 0.3rem; border: 0; margin: 0; padding: 0; min-width: 0; }
  .field > span, legend { font-size: 0.8rem; font-weight: 600; text-transform: uppercase; color: var(--muted); padding: 0; }
  select, input[type='search'], input[type='datetime-local'] {
    background: var(--bg); border: 1px solid var(--line); border-radius: 4px; padding: 0.3rem 0.4rem; min-height: 2.25rem; width: 100%;
  }
  .groups { max-height: 18rem; overflow: auto; border: 1px solid var(--line); border-radius: 4px; background: var(--bg); }
  .system { position: sticky; top: 0; background: var(--surface); font-weight: 600; padding: 0.2rem 0.5rem; }
  .talkgroup { display: flex; gap: 0.4rem; align-items: center; padding: 0.2rem 0.5rem; min-height: 2rem; }
  .number { margin-left: auto; color: var(--muted); font-size: 0.85rem; }
  .check { display: flex; gap: 0.4rem; align-items: center; min-height: 2rem; }
  .sub { display: flex; flex-direction: column; font-size: 0.9rem; gap: 0.2rem; }
  .chosen, .hint { margin: 0; font-size: 0.9rem; color: var(--muted); }
  .link { background: none; border: 0; padding: 0; color: var(--accent); text-decoration: underline; cursor: pointer; text-align: left; }
</style>
