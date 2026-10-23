<script lang="ts">
  import { onMount } from 'svelte';
  import { fetchFavorites, fetchFilterChoices, type Favorite, type TalkgroupGroup } from '../api';
  import { tell } from '../notice.svelte';
  import FavoriteEditor from './FavoriteEditor.svelte';
  import { talkgroupNames } from './favorites';
  import { admin, can, why } from './session.svelte';

  let favorites = $state<Favorite[] | null>(null);
  let groups = $state<TalkgroupGroup[]>([]);
  let problem = $state<string | null>(null);
  /** The list being edited, and the one whose Delete waits for a second click. */
  let editing = $state<string | null>(null);
  let deleting = $state<string | null>(null);
  let refusals = $state<Record<string, string>>({});

  async function read() {
    try {
      [favorites, groups] = await Promise.all([fetchFavorites(), fetchFilterChoices().then((c) => c.talkgroup_groups)]);
      problem = null;
    } catch (error) {
      problem = why(error);
    }
  }
  onMount(read);

  const path = (name: string) => `/api/admin/favorites/${encodeURIComponent(name)}`;

  async function save(name: string, talkgroups: number[]) {
    await admin('PUT', path(name), { talkgroups });
    tell(`Saved the list ${name}.`);
    editing = null;
    await read();
  }

  async function remove(name: string) {
    delete refusals[name];
    try {
      await admin('DELETE', path(name));
      tell(`Deleted the list ${name}.`);
      deleting = null;
      await read();
    } catch (error) {
      refusals[name] = why(error);
    }
  }
</script>

<section class="area" aria-labelledby="favorites-title">
  <h1 id="favorites-title">Favorites</h1>
  <p class="lead">The named lists of talkgroups that every listener can choose in the filters.</p>
  {#if problem}<p class="problem" role="alert">The lists could not be read: {problem}</p>{/if}
  {#if favorites === null}
    <p class="empty">Reading…</p>
  {:else if favorites.length === 0}
    <p class="empty">There is no list yet.</p>
  {:else}
    <ul class="lists">
      {#each favorites as favorite (favorite.id)}
        <li aria-labelledby={`favorite-${favorite.id}`}>
          <h2 id={`favorite-${favorite.id}`}>{favorite.name}</h2>
          <p>{talkgroupNames(favorite.talkgroups, groups).join(', ') || 'No talkgroup'}</p>
          {#if editing === favorite.name}
            <FavoriteEditor id={`edit-${favorite.id}`} name={favorite.name} chosen={favorite.talkgroups} {groups}
              onsave={save} oncancel={() => (editing = null)} />
          {:else if can('favorites')}
            <div class="row">
              <button type="button" onclick={() => (editing = favorite.name)}>Edit {favorite.name}</button>
              {#if deleting === favorite.name}
                <button type="button" class="primary" onclick={() => remove(favorite.name)}>Delete {favorite.name} for good</button>
                <button type="button" onclick={() => (deleting = null)}>Keep it</button>
              {:else}
                <button type="button" onclick={() => (deleting = favorite.name)}>Delete {favorite.name}</button>
              {/if}
            </div>
          {/if}
          {#if refusals[favorite.name]}<p class="problem" role="alert">Not deleted: {refusals[favorite.name]}</p>{/if}
        </li>
      {/each}
    </ul>
  {/if}

  {#if can('favorites')}
    <h2>A new list</h2>
    <FavoriteEditor id="new-list" {groups} taken={favorites?.map((f) => f.name) ?? null} onsave={save} />
  {/if}
</section>

<style>
  .lists { list-style: none; margin: 0; padding: 0; display: grid; gap: 0.75rem; }
  .lists li { border: 1px solid var(--line); border-radius: 6px; padding: 0.7rem 0.9rem; background: var(--surface); }
  .lists h2 { margin: 0 0 0.2rem; font-size: 1.05rem; overflow-wrap: anywhere; }
  .lists p { margin: 0.2rem 0 0.5rem; overflow-wrap: anywhere; }
</style>
