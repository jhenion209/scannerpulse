import { describe, expect, it } from 'vitest';
import { rowClickPlays } from './rowPlay';

/** A row like the call list's: the edge's play button, plain cells, More, the actions menu and a selection box. */
function row() {
  const tr = document.createElement('tr');
  tr.innerHTML = `
    <td class="c-play"><button type="button" aria-label="Play Fire at 10:00:00"></button></td>
    <td class="c-select"><input type="checkbox" /></td>
    <td class="c-talkgroup">Fire</td>
    <td class="c-text"><div class="line"><span class="snippet">Engine 7</span><button type="button" class="more">More</button></div></td>
    <td class="c-actions"><details><summary>⋯</summary><div><button type="button">Edit text</button></div></details></td>`;
  return tr;
}

describe('a click on a call row', () => {
  it('plays the call anywhere outside its controls', () => {
    const tr = row();
    expect(rowClickPlays(tr.querySelector('.c-talkgroup'), '')).toBe(true);
    expect(rowClickPlays(tr.querySelector('.snippet'), '')).toBe(true);
    expect(rowClickPlays(tr.querySelector('.line'), '')).toBe(true);
    expect(rowClickPlays(tr, '')).toBe(true);
  });

  it('leaves the row controls their own click', () => {
    const tr = row();
    expect(rowClickPlays(tr.querySelector('.more'), '')).toBe(false);
    expect(rowClickPlays(tr.querySelector('.c-play button'), '')).toBe(false);
    expect(rowClickPlays(tr.querySelector('summary'), '')).toBe(false);
    expect(rowClickPlays(tr.querySelector('details button'), '')).toBe(false);
    expect(rowClickPlays(tr.querySelector('input'), '')).toBe(false);
  });

  it('does not play when the click ends a text selection, or hits no element', () => {
    const tr = row();
    expect(rowClickPlays(tr.querySelector('.snippet'), 'Engine')).toBe(false);
    expect(rowClickPlays(null, '')).toBe(false);
  });
});
