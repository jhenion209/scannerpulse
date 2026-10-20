<script lang="ts">
  // The guide through the install's conversion to high availability, at the
  // top of the Layout area: every step, the one to take now opened with what
  // it does, what to run on which machine, and the root helper's button for a
  // change on this machine, read again every 15 s. On one machine with no
  // plan yet, how to begin.
  import { onMount } from 'svelte';
  import HelperAction from './HelperAction.svelte';
  import { planText, progressText, stateText, type ConvertInfo } from './convert';
  import { admin, why } from './session.svelte';

  /** How often the guide is read again while the area is open. */
  const GUIDE_EVERY_MS = 15_000;

  let { one }: { one: boolean } = $props();

  let info = $state<ConvertInfo | null>(null);
  let problem = $state<string | null>(null);

  async function read(poll = false) {
    try {
      info = await admin<ConvertInfo>('GET', '/api/admin/convert', undefined, { poll });
      problem = null;
    } catch (error) {
      problem = why(error);
    }
  }

  onMount(() => {
    void read();
    const timer = setInterval(() => void read(true), GUIDE_EVERY_MS);
    return () => clearInterval(timer);
  });
</script>

{#if problem}<p class="problem" role="alert">The conversion's guide could not be read: {problem}</p>{/if}
{#if info?.plan && info.guide}
  {@const guide = info.guide}
  <h2>Conversion to high availability</h2>
  <p class="lead">{progressText(guide)}</p>
  {#if guide.complete}
    <ul class="checks">
      {#each guide.checks as check (check.text)}
        <li>{check.text}: yes</li>
      {/each}
    </ul>
  {:else}
    <ol class="guide">
      {#each guide.steps as step (step.id)}
        <li class={step.state} aria-current={step.state === 'now' ? 'step' : undefined}>
          <span class="title">{step.title}</span> <span class="state">({stateText(step.state)})</span>
          {#if step.state === 'now'}
            <p>{step.text}</p>
            {#if step.run.length > 0}
              <p>On {step.on}:</p>
              <pre><code>{step.run.join('\n')}</code></pre>
            {/if}
            {#if step.id === 'final-check'}
              <ul class="checks">
                {#each guide.checks as check (check.text)}
                  <li class={check.ok ? '' : 'flag'}>{check.text}: {check.ok ? 'yes' : 'not yet'}</li>
                {/each}
              </ul>
            {/if}
            {#if step.ask}
              {#key step.id}
                <HelperAction kind={step.ask.kind} label={step.ask.label} question={step.ask.question} />
              {/key}
            {/if}
          {/if}
        </li>
      {/each}
    </ol>
  {/if}
{:else if info && one}
  <p>{planText}</p>
{/if}

<style>
  .guide { display: grid; gap: 0.5rem; padding-left: 1.5rem; }
  .guide li.done .title, .guide li.later .title { opacity: 0.7; }
  .guide li.now .title { font-weight: 600; }
  .state { font-size: 0.85rem; opacity: 0.8; }
  .checks { margin: 0.4rem 0; }
  pre { margin: 0.4rem 0 0; padding: 0.6rem 0.8rem; border-radius: 4px; background: var(--panel, rgba(127, 127, 127, 0.1)); }
  code { font-size: 0.9rem; white-space: pre-wrap; overflow-wrap: anywhere; }
</style>
