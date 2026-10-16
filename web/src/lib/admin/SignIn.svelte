<script lang="ts">
  import { onMount } from 'svelte';
  import { apiError } from '../api';
  import { session, signIn, why } from './session.svelte';

  let methods = $state<{ password: boolean; oidc: boolean } | null>(null);
  let login = $state('');
  let password = $state('');
  let busy = $state(false);
  let refusal = $state<string | null>(null);

  onMount(async () => {
    try {
      const response = await fetch('/api/auth/methods', { headers: { accept: 'application/json' } });
      if (!response.ok) throw await apiError(response);
      methods = await response.json();
    } catch (error) {
      // The form still shows: the sign-in itself says what is wrong.
      refusal = `The ways to sign in could not be read: ${why(error)}`;
      methods = { password: true, oidc: false };
    }
  });

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (busy) return;
    busy = true;
    refusal = null;
    try {
      await signIn(login.trim(), password);
      password = '';
    } catch (error) {
      refusal = why(error);
    } finally {
      busy = false;
    }
  }
</script>

<main class="signin">
  <h1>ScannerPulse · Admin</h1>
  {#if session.ended}<p class="note" role="status">The session has ended. Sign in again.</p>{/if}
  {#if methods?.password}
    <form onsubmit={submit} aria-label="Sign in">
      <label for="signin-login">Name</label>
      <input id="signin-login" autocomplete="username" required bind:value={login} />
      <label for="signin-password">Password</label>
      <input id="signin-password" type="password" autocomplete="current-password" required bind:value={password} />
      <button type="submit" class="primary" disabled={busy}>{busy ? 'Signing in…' : 'Sign in'}</button>
    </form>
  {/if}
  {#if refusal}<p class="problem" role="alert">{refusal}</p>{/if}
  {#if methods?.oidc}
    <p><a class="button" href="/api/auth/oidc">Sign in with single sign-on</a></p>
  {/if}
  {#if methods && !methods.password && !methods.oidc}
    <p class="problem" role="alert">
      No way to sign in is on. On the server, <code>scannerpulse admin password-signin on</code> turns password sign-in back on.
    </p>
  {/if}
  <p><a href="/">Back to the calls</a></p>
</main>

<style>
  .signin { max-width: 24rem; margin: 3rem auto; padding: 0 1rem; }
  h1 { font-size: 1.35rem; margin: 0 0 1rem; }
  form { display: grid; gap: 0.35rem; }
  label { font-weight: 600; margin-top: 0.4rem; }
  form button { margin-top: 0.8rem; justify-content: center; }
  .note { padding: 0.4rem 0.6rem; border-radius: 4px; background: var(--surface); border: 1px solid var(--line); }
  a { color: var(--accent); }
  .button { color: var(--text); }
</style>
