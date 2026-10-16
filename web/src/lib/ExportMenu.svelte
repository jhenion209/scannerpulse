<script lang="ts">
  import { tick } from 'svelte';
  import { fetchAudio, fetchExcel, type Call } from './api';
  import type { ColumnId } from './columns';
  import Dropdown from './Dropdown.svelte';
  import { audioBody, csvText, exportName, exportQuery, pageTable, saveFile } from './exporting';
  import { tell } from './notice.svelte';

  let { calls, selecting, chosen, columns, zone, params, printChosen = $bindable(false) }: {
    /** The page on screen. */
    calls: Call[];
    /** Selection mode is on: the page's exports and call audio then cover the ticked calls only. */
    selecting: boolean;
    /** The calls ticked in selection mode. */
    chosen: number[];
    columns: ColumnId[];
    /** The site's time zone, for the file names and the CSV's times. */
    zone: string;
    /** The list's query, for the files built on the server from every match. */
    params: () => URLSearchParams;
    /** Printing the chosen calls only: the page hides the others on paper. */
    printChosen?: boolean;
  } = $props();

  // Excel and call audio are built on the server from every match of the
  // list (or, for audio, the chosen calls); CSV, PDF and print are the page
  // on screen, or the chosen calls when some are ticked. A file of the page
  // or the chosen calls is named by those calls' own time range, as the
  // server names a file of chosen calls.
  let busy = $state(false);
  const chosenCount = $derived(selecting ? chosen.length : 0);
  const scope = $derived(selecting ? `${chosenCount} selected` : 'this page');
  // Selection on and nothing ticked: no export of the page or of the audio
  // (nothing never means everything). Excel is every match either way.
  const nothing = $derived(selecting && chosenCount === 0);
  const rows = () => (selecting && chosen.length ? calls.filter((c) => chosen.includes(c.id)) : calls);
  const oldestFirst = (list: Call[]) => [...list].sort((a, b) => Date.parse(a.started_at) - Date.parse(b.started_at));
  const pageName = (ext: string) => exportName(oldestFirst(rows()), null, null, zone, ext);
  const why = (error: unknown) => (error instanceof Error ? error.message : String(error));
  const NOTHING = 'Nothing is selected: tick calls first.';

  async function excel() {
    busy = true;
    tell('Making the Excel file of every match…');
    try {
      const file = await fetchExcel(exportQuery(params()));
      saveFile(file.blob, file.name);
      tell(file.note ?? `Saved ${file.name}.`);
    } catch (error) {
      tell(`The Excel file could not be made: ${why(error)}`);
    } finally {
      busy = false;
    }
  }

  function csv() {
    if (nothing) return tell(NOTHING);
    const name = pageName('csv');
    saveFile(new Blob([csvText(pageTable(rows(), columns, zone, location.origin))], { type: 'text/csv' }), name);
    tell(`Saved ${name}.`);
  }

  /** Prints the page (or the chosen calls); the file name the browser offers
   * for a PDF is the export name, as the page's title. */
  async function printPage(kind: 'pdf' | 'print') {
    if (nothing) return tell(NOTHING);
    const title = document.title;
    printChosen = selecting && chosen.length > 0;
    document.title = pageName('pdf').replace(/\.pdf$/, '');
    await tick();
    const restore = () => {
      document.title = title;
      printChosen = false;
    };
    window.addEventListener('afterprint', restore, { once: true });
    window.print();
    if (kind === 'pdf') tell('Choose “Save as PDF” in the print dialog to keep a PDF.');
  }

  async function audio() {
    if (nothing) return tell(NOTHING);
    busy = true;
    const ids = selecting && chosen.length ? chosen : undefined;
    tell(ids ? `Joining the audio of ${ids.length} chosen calls…` : 'Joining the audio of every match…');
    try {
      const file = await fetchAudio(audioBody(params(), ids));
      saveFile(file.blob, file.name);
      tell(file.note ?? `Saved ${file.name}.`);
    } catch (error) {
      tell(`The call audio could not be made: ${why(error)}`);
    } finally {
      busy = false;
    }
  }
</script>

<Dropdown summary="Export" class="export">
  {#snippet children(close)}
    <button type="button" disabled={busy} onclick={() => { close(); excel(); }}>Excel — every match</button>
    <button type="button" disabled={nothing} onclick={() => { close(); csv(); }}>CSV — {scope}</button>
    <button type="button" disabled={nothing} onclick={() => { close(); printPage('pdf'); }}>PDF — {scope}</button>
    <button type="button" disabled={nothing} onclick={() => { close(); printPage('print'); }}>Print — {scope}</button>
    <button type="button" disabled={busy || nothing} onclick={() => { close(); audio(); }}>
      Call audio — {selecting ? `${chosenCount} selected` : 'every match'}
    </button>
    <p class="hint">PDF opens the print dialog: choose “Save as PDF”.</p>
  {/snippet}
</Dropdown>

<style>
  button {
    text-align: left; min-height: 2.25rem; padding: 0 0.6rem; border: 0; border-radius: 4px;
    background: none; cursor: pointer; white-space: nowrap;
  }
  button:hover:not(:disabled) { background: var(--surface); }
  button:disabled { opacity: 0.55; cursor: default; }
  .hint { margin: 0.3rem 0.6rem 0.1rem; font-size: 0.85rem; color: var(--muted); max-width: 16rem; }
</style>
