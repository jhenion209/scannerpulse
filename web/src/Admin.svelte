<script lang="ts">
  import { onMount, type Component } from 'svelte';
  import AccountsArea from './lib/admin/AccountsArea.svelte';
  import { AREAS, areaOf, type AreaId } from './lib/admin/areas';
  import { activeCount } from './lib/admin/jobs';
  import { followJobs, jobs } from './lib/admin/jobs.svelte';
  import FavoritesArea from './lib/admin/FavoritesArea.svelte';
  import JobsArea from './lib/admin/JobsArea.svelte';
  import { checkSession, session, signOut, why } from './lib/admin/session.svelte';
  import MaintenanceArea from './lib/admin/MaintenanceArea.svelte';
  import ReportsArea from './lib/admin/ReportsArea.svelte';
  import SettingsArea from './lib/admin/SettingsArea.svelte';
  import SignIn from './lib/admin/SignIn.svelte';
  import StatusArea from './lib/admin/StatusArea.svelte';
  import { notice, tell } from './lib/notice.svelte';
  import { loadPrefs } from './lib/prefs';
  import { startStatus } from './lib/status.svelte';
  import ThemeSwitch from './lib/ThemeSwitch.svelte';
  import TranscriptionArea from './lib/admin/TranscriptionArea.svelte';
  import ViewersArea from './lib/admin/ViewersArea.svelte';

  // Each area of the side menu, by its id.
  const PAGES: Record<AreaId, Component> = { status: StatusArea, maintenance: MaintenanceArea, transcription: TranscriptionArea, reports: ReportsArea, accounts: AccountsArea, settings: SettingsArea,
    favorites: FavoritesArea, viewers: ViewersArea, jobs: JobsArea };

  const prefs = $state(loadPrefs());
  let area = $state<AreaId>(areaOf(location.hash));
  let menuOpen = $state(false);
  let problem = $state<string | null>(null);
  const Page = $derived(PAGES[area]);

  // The area follows the address (`#status`), so a link opens an area and
  // the browser's Back returns to the one before.
  onMount(() => {
    const follow = () => {
      area = areaOf(location.hash);
      menuOpen = false;
    };
    window.addEventListener('hashchange', follow);
    return () => window.removeEventListener('hashchange', follow);
  });
  // The status gives the site's time zone, in which every time is shown.
  onMount(startStatus);
  onMount(() => void check());
  // While an admin is signed in, the jobs are followed for the menu's badge.
  $effect(() => {
    if (session.login) return followJobs();
  });
  const running = $derived(jobs.list ? activeCount(jobs.list) : 0);

  async function check() {
    problem = null;
    try {
      await checkSession();
    } catch (error) {
      problem = why(error);
    }
  }

  async function leave() {
    try {
      await signOut();
    } catch (error) {
      tell(`Signing out failed: ${why(error)}`);
    }
  }
</script>

{#if problem}
  <main class="signin">
    <h1>ScannerPulse · Admin</h1>
    <p class="problem" role="alert">The server could not say who is signed in: {problem}</p>
    <button type="button" onclick={check}>Try again</button>
  </main>
{:else if !session.checked}
  <main class="signin" aria-busy="true"><p>Reading…</p></main>
{:else if !session.login}
  <SignIn />
{:else}
  <div class="shell" class:menu-open={menuOpen}>
    <header class="bar">
      <button type="button" class="menu" onclick={() => (menuOpen = !menuOpen)} aria-controls="areas" aria-expanded={menuOpen}>
        Menu
      </button>
      <span class="name">ScannerPulse · Admin</span>
      <span class="spacer"></span>
      <span class="who">{session.login}</span>
      <button type="button" class="out" onclick={leave}>Sign out</button>
      <ThemeSwitch bind:theme={prefs.theme} />
      <a class="calls" href="/">Calls</a>
    </header>
    <nav id="areas" class="side" aria-label="Admin areas">
      {#each AREAS as item (item.id)}
        {@const count = item.id === 'jobs' ? running : 0}
        <a href={`#${item.id}`} aria-current={area === item.id ? 'page' : undefined}
          aria-label={count ? `${item.label}, ${count} under way` : undefined}
          onclick={() => (menuOpen = false)}>
          {item.label}{#if count}<span class="badge" aria-hidden="true">{count}</span>{/if}
        </a>
      {/each}
    </nav>
    <main class="main">
      {#key area}<Page />{/key}
    </main>
  </div>
{/if}
<p class="notice" role="status">{notice.text ?? ''}</p>

<style>
  .signin { max-width: 24rem; margin: 3rem auto; padding: 0 1rem; }
  .signin h1 { font-size: 1.35rem; }
  .shell {
    display: grid; min-height: 100vh; min-height: 100dvh;
    grid-template: 'bar bar' auto 'side main' 1fr / 13rem 1fr;
  }
  .bar {
    grid-area: bar; display: flex; flex-wrap: wrap; gap: 0.75rem; row-gap: 0.4rem; align-items: center;
    padding: 0.5rem 1rem; background: var(--bar); color: var(--bar-text);
  }
  .bar button {
    background: none; color: var(--bar-text); border: 1px solid currentColor; border-radius: 4px;
    padding: 0.25rem 0.6rem; min-height: 2.25rem; cursor: pointer;
  }
  .menu { display: none; }
  .name { font-weight: 700; color: var(--accent-strong); }
  .spacer { flex: 1; }
  .who { overflow-wrap: anywhere; }
  .calls { color: var(--bar-text); }
  .side {
    grid-area: side; display: flex; flex-direction: column; gap: 0.15rem; padding: 0.75rem 0.5rem;
    background: var(--surface); border-right: 1px solid var(--line);
  }
  .side a { padding: 0.45rem 0.7rem; border-radius: 4px; color: var(--text); text-decoration: none; }
  .side a:hover { background: var(--bg); }
  .side a[aria-current='page'] { background: var(--bg); color: var(--accent); font-weight: 700; box-shadow: inset 3px 0 0 var(--accent); }
  .badge {
    margin-left: 0.4rem; padding: 0 0.45rem; border-radius: 999px; background: var(--accent); color: var(--bg);
    font-weight: 700;
  }
  .main { grid-area: main; min-width: 0; }
  .notice {
    position: fixed; bottom: 1rem; left: 50%; transform: translateX(-50%); z-index: 30; margin: 0;
    max-width: calc(100% - 2rem); overflow-wrap: anywhere; pointer-events: none;
  }
  .notice:not(:empty) {
    padding: 0.4rem 0.6rem; border-radius: 4px; background: var(--surface);
    border: 1px solid var(--field-border); box-shadow: 0 4px 14px rgb(0 0 0 / 0.25);
  }
  @media (max-width: 760px) {
    .shell { grid-template: 'bar' auto 'main' 1fr / 1fr; }
    .shell.menu-open { grid-template: 'bar' auto 'side' auto 'main' 1fr / 1fr; }
    .menu { display: inline-block; }
    .side { display: none; }
    .menu-open .side { display: flex; border-right: 0; border-bottom: 1px solid var(--line); }
  }
</style>
