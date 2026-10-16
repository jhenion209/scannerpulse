<script lang="ts">
  import { onMount } from 'svelte';
  import { callTime } from '../format';
  import { tell } from '../notice.svelte';
  import { siteZone } from '../status.svelte';
  import { SIGN_IN_KEYS, signsInWith, type Account } from './accounts';
  import PasswordForm from './PasswordForm.svelte';
  import { admin, session, why } from './session.svelte';
  import SettingField from './SettingField.svelte';
  import type { Setting } from './settings';

  let accounts = $state<Account[] | null>(null);
  let settings = $state<Setting[]>([]);
  let problem = $state<string | null>(null);
  /** The account whose new-password form is open. */
  let renewing = $state<string | null>(null);
  /** A refusal beside the account whose button asked for it. */
  let refusals = $state<Record<string, string>>({});

  async function read() {
    try {
      accounts = await admin<Account[]>('GET', '/api/admin/accounts');
      problem = null;
    } catch (error) {
      problem = why(error);
    }
  }

  async function readSettings() {
    try {
      const all = await admin<Setting[]>('GET', '/api/admin/settings');
      settings = all.filter((s) => SIGN_IN_KEYS.includes(s.key));
    } catch (error) {
      problem = why(error);
    }
  }

  onMount(() => {
    void read();
    void readSettings();
  });

  const path = (login: string, action: string) => `/api/admin/accounts/${encodeURIComponent(login)}/${action}`;

  async function setDisabled(account: Account, disable: boolean) {
    delete refusals[account.login];
    try {
      await admin('POST', path(account.login, disable ? 'disable' : 'enable'));
      tell(`${account.login} is ${disable ? 'disabled' : 'enabled'}.`);
      await read();
    } catch (error) {
      refusals[account.login] = why(error);
    }
  }

  async function renew(_login: string, password: string) {
    const login = renewing!;
    await admin('POST', path(login, 'password'), { password });
    tell(`${login} has a new password.`);
    renewing = null;
  }

  async function add(login: string, password: string) {
    await admin('POST', '/api/admin/accounts', { login, password });
    tell(`${login} is added.`);
    await read();
  }

  const when = (at: string | null) => (at === null ? 'never' : callTime(at, new Date(), siteZone()));
</script>

<section class="area" aria-labelledby="accounts-title">
  <h1 id="accounts-title">Accounts</h1>
  <p class="lead">
    The admins, who sign in on this page. At least one must stay enabled. A new password, or disabling an
    account, ends that account's sessions.
  </p>
  {#if problem}<p class="problem" role="alert">The accounts could not be read: {problem}</p>{/if}
  {#if accounts === null}
    <p class="empty">Reading…</p>
  {:else}
    <ul class="accounts">
      {#each accounts as account (account.login)}
        {@const you = account.login === session.login}
        <li aria-labelledby={`account-${account.login}`}>
          <h2 id={`account-${account.login}`}>{account.login}{you ? ' (you)' : ''}</h2>
          <p>
            Signs in with {signsInWith(account)} ·
            {#if account.disabled}<span class="flag">disabled</span>{:else}enabled{/if}
          </p>
          <dl class="facts">
            <dt>Added</dt><dd>{when(account.created_at)}</dd>
            <dt>Last signed in</dt><dd>{when(account.last_sign_in)}</dd>
          </dl>
          <div class="row">
            <button type="button" onclick={() => (renewing = renewing === account.login ? null : account.login)}
              aria-expanded={renewing === account.login}>New password for {account.login}</button>
            {#if account.disabled}
              <button type="button" onclick={() => setDisabled(account, false)}>Enable {account.login}</button>
            {:else}
              <button type="button" onclick={() => setDisabled(account, true)}>
                Disable {account.login}{you ? ' (signs you out)' : ''}
              </button>
            {/if}
          </div>
          {#if renewing === account.login}
            <PasswordForm id={`renew-${account.login}`} submitLabel="Save the password" onsubmit={renew}
              oncancel={() => (renewing = null)} />
          {/if}
          {#if refusals[account.login]}<p class="problem" role="alert">Not changed: {refusals[account.login]}</p>{/if}
        </li>
      {/each}
    </ul>
  {/if}

  <h2>Add an admin</h2>
  <PasswordForm id="add" withName submitLabel="Add the admin" onsubmit={add} />

  <h2>Signing in</h2>
  <p>
    Password sign-in can be turned off once single sign-on works (the command line turns it on again).
    Single sign-on needs these four settings, and the host's <code>SCANNERPULSE_OIDC_CLIENT_SECRET</code>,
    which is never stored here.
  </p>
  {#each settings as setting (`${setting.key}|${setting.updated_at}`)}
    <SettingField {setting} onchanged={readSettings} />
  {/each}
</section>

<style>
  .accounts { list-style: none; margin: 0; padding: 0; display: grid; gap: 0.75rem; }
  .accounts li { border: 1px solid var(--line); border-radius: 6px; padding: 0.7rem 0.9rem; background: var(--surface); }
  .accounts h2 { margin: 0 0 0.2rem; font-size: 1.05rem; overflow-wrap: anywhere; }
  .accounts p { margin: 0.2rem 0; }
  .facts { margin: 0.4rem 0 0.6rem; }
</style>
