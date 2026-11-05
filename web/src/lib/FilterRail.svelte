<script lang="ts">
  import type { Favorite, FilterChoices } from './api';
  import type { Filters } from './filters';
  import TalkgroupPicker from './TalkgroupPicker.svelte';

  let {
    filters = $bindable(),
    paused = $bindable(false),
    choices,
    favorites,
  }: { filters: Filters; paused?: boolean; choices: FilterChoices; favorites: Favorite[] } = $props();

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

  <TalkgroupPicker bind:chosen={() => filters.talkgroups, (talkgroups) => set({ talkgroups })} {choices} />

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
  select, input[type='datetime-local'] {
    background: var(--bg); border: 1px solid var(--field-border); border-radius: 4px; padding: 0.3rem 0.4rem; min-height: 2.25rem; width: 100%;
  }
  .check { display: flex; gap: 0.4rem; align-items: center; min-height: 2rem; }
  .sub { display: flex; flex-direction: column; font-size: 0.9rem; gap: 0.2rem; }
  .hint { margin: 0; font-size: 0.9rem; color: var(--muted); }
  .link { background: none; border: 0; padding: 0; color: var(--accent); text-decoration: underline; cursor: pointer; text-align: left; }
</style>
