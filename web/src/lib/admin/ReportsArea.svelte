<script lang="ts">
  import { onMount } from 'svelte';
  import { admin, why } from './session.svelte';
  import { hourly, type Quiet, type Traffic } from './reports';
  import TrafficChart from './TrafficChart.svelte';

  let traffic = $state<Traffic | null>(null);
  let problem = $state<string | null>(null);
  onMount(async () => {
    try {
      traffic = await admin<Traffic>('GET', '/api/admin/reports/traffic');
    } catch (error) {
      problem = why(error);
    }
  });

  const weekday = $derived(traffic ? hourly(traffic.weekday) : []);
  const weekend = $derived(traffic ? hourly(traffic.weekend) : []);
  // One scale for both charts, so they compare.
  const top = $derived(Math.max(0, ...weekday, ...weekend));
  const count = (n: number) => n.toLocaleString('en-US');
  const share = (part: number, whole: number) => (whole > 0 ? ` (${Math.round((part / whole) * 100)}%)` : '');
  const clock = (hour: number) => `${String(hour).padStart(2, '0')}:00`;
</script>

{#snippet quietest(label: string, windows: Quiet[])}
  <div>
    <h3>{label[0].toUpperCase() + label.slice(1)}</h3>
    {#if windows.length === 0}
      <p class="empty">No call in these days.</p>
    {:else}
      <ul aria-label={`The quietest hours on ${label}`}>
        {#each windows as w (w.start)}<li>{w.start} to {w.end}: {w.calls_per_day.toFixed(1)} calls a day</li>{/each}
      </ul>
    {/if}
  </div>
{/snippet}

<section class="area" aria-labelledby="reports-title">
  <h1 id="reports-title">Reports</h1>
  <p class="lead">
    The traffic: calls a day in each hour, over the full days before today, in the site's zone. The server makes
    it again at most every 10 minutes, since it reads months of calls.
  </p>
  {#if problem}
    <p class="problem" role="alert">The report could not be made: {problem}</p>
  {:else if traffic === null}
    <p class="empty">Reading…</p>
  {:else}
    <p>
      {count(traffic.calls)} calls in the {traffic.days} days before today ({traffic.weekdays} weekdays,
      {traffic.weekend_days} weekend days); times in {traffic.time_zone}.
    </p>
    <dl class="facts">
      <dt>Transcribed</dt><dd>{count(traffic.transcribed)}{share(traffic.transcribed, traffic.calls)}</dd>
      <dt>No speech</dt><dd>{count(traffic.no_speech)}{share(traffic.no_speech, traffic.calls)}</dd>
    </dl>

    <h2>Each hour</h2>
    <div class="charts">
      <TrafficChart title="Weekdays" hours={weekday} {top} />
      <TrafficChart title="Weekend days" hours={weekend} {top} />
    </div>
    <details>
      <summary>Every hour's figures</summary>
      <table>
        <caption class="visually-hidden">Calls a day in each hour</caption>
        <thead><tr><th scope="col">Hour</th><th scope="col" class="num">Weekdays</th><th scope="col" class="num">Weekend days</th></tr></thead>
        <tbody>
          {#each weekday as value, hour (hour)}
            <tr><td>{clock(hour)}</td><td class="num">{value.toFixed(1)}</td><td class="num">{weekend[hour].toFixed(1)}</td></tr>
          {/each}
        </tbody>
      </table>
    </details>

    <h2>The quietest hours</h2>
    <p>The hours with the fewest calls, the quietest first, for planning maintenance.</p>
    <div class="charts">
      {@render quietest('weekdays', traffic.quietest_weekday)}
      {@render quietest('weekend days', traffic.quietest_weekend)}
    </div>
  {/if}
</section>

<style>
  .charts { display: grid; grid-template-columns: repeat(auto-fit, minmax(16rem, 1fr)); gap: 1rem 2rem; }
  details { margin-top: 0.75rem; max-width: 30rem; }
  summary { cursor: pointer; }
  h3 { margin: 0.5rem 0 0.3rem; font-size: 1rem; }
  ul { margin: 0; padding-left: 1.2rem; }
</style>
