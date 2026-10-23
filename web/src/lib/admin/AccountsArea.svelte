<script lang="ts">
  import { onMount } from 'svelte';
  import { callTime } from '../format';
  import { tell } from '../notice.svelte';
  import { siteZone } from '../status.svelte';
  import { SIGN_IN_KEYS, grouped, roleWords, signsInWith, type Account, type Accounts, type Fn } from './accounts';
  import PasswordForm from './PasswordForm.svelte';
  import { admin, isSuper, session, why } from './session.svelte';
  import SettingField from './SettingField.svelte';
  import type { Setting } from './settings';

  let accounts = $state<Account[] | null>(null);
  let functions = $state<Fn[]>([]);
  let settings = $state<Setting[]>([]);
  let problem = $state<string | null>(null);
  /** The account whose new-password form is open. */
  let renewing = $state<string | null>(null);
  /** A refusal beside the account whose button asked for it. */
  let refusals = $state<Record<string, string>>({});
  /** The role an admin added on this page gets. */
  let newRole = $state<'associate' | 'super'>('associate');

  async function read() {
    try {
      const answer = await admin<Accounts>('GET', '/api/admin/accounts');
      accounts = answer.accounts;
      functions = answer.functions;
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
    if (isSuper()) void readSettings();
  });

  const path = (login: string, action: string) => `/api/admin/accounts/${encodeURIComponent(login)}/${action}`;

  /** A change of one account; a refusal shows beside it. Either way the
   * accounts are read again. */
  async function change(account: Account, done: string, send: () => Promise<unknown>) {
    delete refusals[account.login];
    try {
      await send();
      tell(done);
    } catch (error) {
      refusals[account.login] = why(error);
    }
    await read();
  }

  const setDisabled = (account: Account, disable: boolean) =>
    change(account, `${account.login} is ${disable ? 'disabled' : 'enabled'}.`, () =>
      admin('POST', path(account.login, disable ? 'disable' : 'enable')));

  const setRole = (account: Account, role: 'super' | 'associate') =>
    change(account, `${account.login} is ${role === 'super' ? 'a super admin' : 'an associate'}.`, () =>
      admin('PUT', path(account.login, 'role'), { role }));

  async function setFunction(account: Account, fn: Fn, box: HTMLInputElement) {
    const on = box.checked;
    await change(account, `${fn.label} is ${on ? 'on' : 'off'} for ${account.login}.`, () =>
      admin(on ? 'PUT' : 'DELETE', path(account.login, `functions/${fn.name}`)));
    // `checked` is one-way: nothing writes the stored state back to a box the person toggled.
    const stored = accounts?.find((a) => a.login === account.login);
    if (stored) box.checked = stored.functions.includes(fn.name);
  }

  async function renew(_login: string, password: string) {
    const login = renewing!;
    await admin('POST', path(login, 'password'), { password });
    tell(`${login} has a new password.`);
    renewing = null;
  }

  async function add(login: string, password: string) {
    await admin('POST', '/api/admin/accounts', { login, password, role: newRole });
    tell(`${login} is added, as ${newRole === 'super' ? 'a super admin' : 'an associate with every function off'}.`);
    await read();
  }

  const when = (at: string | null) => (at === null ? 'never' : callTime(at, new Date(), siteZone()));
</script>

<section class="area" aria-labelledby="accounts-title">
  <h1 id="accounts-title">Accounts</h1>
  {#if isSuper()}
    <p class="lead">
      The admins, who sign in on this page. A super admin may change everything; an associate only the
      functions turned on below, each for that person. At least one super admin must stay enabled. A new
      password, or disabling an account, ends that account's sessions.
    </p>
  {:else}
    <p class="lead">
      Your account. You are an associate: you may change only what a super admin turned on for you.
    </p>
  {/if}
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
            {roleWords(account)} · signs in with {signsInWith(account)} ·
            {#if account.disabled}<span class="flag">disabled</span>{:else}enabled{/if}
          </p>
          <dl class="facts">
            <dt>Added</dt><dd>{when(account.created_at)}</dd>
            <dt>Last signed in</dt><dd>{when(account.last_sign_in)}</dd>
          </dl>
          {#if isSuper() && account.role === 'associate'}
            <div class="switches">
              {#each grouped(functions) as group (group.group)}
                <fieldset>
                  <legend>{group.group}</legend>
                  {#each group.functions as fn (fn.name)}
                    <label>
                      <input type="checkbox" checked={account.functions.includes(fn.name)}
                        onchange={(event) => setFunction(account, fn, event.currentTarget)}
                        aria-label={`${fn.label} for ${account.login}`} />
                      {fn.label}
                    </label>
                  {/each}
                </fieldset>
              {/each}
            </div>
          {/if}
          <div class="row">
            <!-- An associate signing in only through the provider has no password to change. -->
            {#if (you && account.password) || isSuper()}
              <button type="button" onclick={() => (renewing = renewing === account.login ? null : account.login)}
                aria-expanded={renewing === account.login}>New password for {account.login}</button>
            {/if}
            {#if isSuper()}
              {#if account.role === 'associate'}
                <button type="button" onclick={() => setRole(account, 'super')}>Make {account.login} a super admin</button>
              {:else}
                <button type="button" onclick={() => setRole(account, 'associate')}>
                  Make {account.login} an associate{you ? ' (you)' : ''}
                </button>
              {/if}
              {#if account.disabled}
                <button type="button" onclick={() => setDisabled(account, false)}>Enable {account.login}</button>
              {:else}
                <button type="button" onclick={() => setDisabled(account, true)}>
                  Disable {account.login}{you ? ' (signs you out)' : ''}
                </button>
              {/if}
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

  {#if isSuper()}
    <h2>Add an admin</h2>
    <fieldset class="role">
      <legend>The new admin is</legend>
      <label><input type="radio" name="new-role" value="associate" bind:group={newRole} /> an associate, with every function off</label>
      <label><input type="radio" name="new-role" value="super" bind:group={newRole} /> a super admin</label>
    </fieldset>
    <PasswordForm id="add" withName submitLabel="Add the admin" onsubmit={add} />

    <h2>Signing in</h2>
    <p>
      Password sign-in can be turned off once single sign-on works (the command line turns it on again).
      Single sign-on needs these four settings, and the host's <code>SCANNERPULSE_OIDC_CLIENT_SECRET</code>,
      which is never stored here. Someone signing in through it for the first time is an associate with every
      function off.
    </p>
    {#each settings as setting (`${setting.key}|${setting.updated_at}`)}
      <SettingField {setting} onchanged={readSettings} />
    {/each}
  {/if}
</section>

<style>
  .accounts { list-style: none; margin: 0; padding: 0; display: grid; gap: 0.75rem; }
  .accounts li { border: 1px solid var(--line); border-radius: 6px; padding: 0.7rem 0.9rem; background: var(--surface); }
  .accounts h2 { margin: 0 0 0.2rem; font-size: 1.05rem; overflow-wrap: anywhere; }
  .accounts p { margin: 0.2rem 0; }
  .facts { margin: 0.4rem 0 0.6rem; }
  .switches { display: grid; grid-template-columns: repeat(auto-fill, minmax(min(100%, 12rem), 1fr)); gap: 0.5rem; margin: 0 0 0.6rem; }
  .switches fieldset, .role { border: 1px solid var(--line); border-radius: 6px; padding: 0.3rem 0.6rem 0.5rem; margin: 0; min-width: 0; }
  .switches label, .role label { display: flex; gap: 0.4rem; align-items: baseline; }
  .role { margin-bottom: 0.6rem; }
</style>
