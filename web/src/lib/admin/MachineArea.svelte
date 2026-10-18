<script lang="ts">
  import { onMount } from 'svelte';
  import { callTime } from '../format';
  import { siteZone } from '../status.svelte';
  import { backupAge, certificateEnd, COMMANDS, kindName, type MachineInfo } from './machine';
  import { admin, why } from './session.svelte';

  let data = $state<MachineInfo | null>(null);
  let problem = $state<string | null>(null);

  async function read() {
    try {
      data = await admin<MachineInfo>('GET', '/api/admin/machine');
      problem = null;
    } catch (error) {
      problem = why(error);
    }
  }

  onMount(() => void read());

  const now = () => new Date();
  const when = (at: string) => callTime(at, now(), siteZone());
</script>

<section class="area" aria-labelledby="machine-title">
  <h1 id="machine-title">This machine</h1>
  <p class="lead">
    {#if data}{data.host}: {/if}what its settings file says. The web service runs without root, so each part is
    changed by a command run on this machine; logs and reverse proxies are in Settings (logs.syslog,
    web.trusted_proxies).
  </p>
  {#if problem}<p class="problem" role="alert">This machine could not be read: {problem}</p>{/if}

  {#if data}
    <h2>Certificate</h2>
    {#if data.certificate === null}
      <p>The site serves plain HTTP on this machine's own address only.</p>
    {:else if data.certificate.error}
      <p class="problem">{data.certificate.error}</p>
    {:else if data.certificate.names && data.certificate.not_after}
      {@const end = certificateEnd(data.certificate.not_after, now())}
      <dl class="facts">
        <dt>For</dt>
        <dd>{data.certificate.names.join(', ')}</dd>
        <dt>Until</dt>
        <dd>{when(data.certificate.not_after)} <span class={end.ok ? '' : 'flag'}>({end.text})</span></dd>
        <dt>Signed by</dt>
        <dd>{data.certificate.self_signed ? 'itself: browsers warn once' : 'an authority'}</dd>
      </dl>
    {/if}
    <p>To serve your own certificate, or go back to this machine's own:</p>
    <pre><code>{COMMANDS.certificate}
{COMMANDS.ownCertificate}</code></pre>

    <h2>Backups</h2>
    {#if data.backups === null}
      <p>The database is one you run: its backups are yours.</p>
    {:else}
      <dl class="facts">
        <dt>Folder</dt>
        <dd>{data.backups.folder}</dd>
        {#each Object.entries(data.backups.kinds) as [kind, info] (kind)}
          {@const age = backupAge(info.newest, now())}
          <dt>{kindName(kind)}</dt>
          <dd>
            <span class={age.ok ? '' : 'flag'}>{age.text}</span>{#if info.newest} ({when(info.newest)}){/if};
            {info.push ? 'a push address is set' : 'no push address'}
          </dd>
        {/each}
      </dl>
      <p>To push to a monitor (such as an Uptime Kuma push monitor) after each backup, or move the backups:</p>
      <pre><code>{COMMANDS.push('pgbackrest')}
{COMMANDS.push('dump')}
{COMMANDS.location}</code></pre>
    {/if}
  {/if}
</section>

<style>
  pre { margin: 0.4rem 0 0; padding: 0.6rem 0.8rem; overflow-x: auto; border-radius: 4px; background: var(--panel, rgba(127, 127, 127, 0.1)); }
  code { font-size: 0.9rem; white-space: pre; }
</style>
