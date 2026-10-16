<script lang="ts">
  import { applyTheme, savePref, type Theme } from './prefs';

  let { theme = $bindable('auto') }: { theme?: Theme } = $props();
  const next: Record<Theme, Theme> = { auto: 'dark', dark: 'light', light: 'auto' };
  const label: Record<Theme, string> = {
    auto: 'Theme: as the device',
    dark: 'Theme: dark',
    light: 'Theme: light',
  };

  function cycle() {
    theme = next[theme];
    applyTheme(theme);
    savePref('theme', theme);
  }
</script>

<button class="theme" type="button" onclick={cycle} title={label[theme]} aria-label={label[theme]}>
  {theme === 'dark' ? '☾' : theme === 'light' ? '☀' : '◐'}
</button>

<style>
  .theme {
    background: transparent; border: 1px solid #ffffff44; border-radius: 999px;
    color: var(--bar-text); min-width: 2.25rem; min-height: 2.25rem; cursor: pointer;
  }
</style>
