<script lang="ts">
  // One day's traffic as 24 hourly bars. The bars are a picture only: the
  // area's table gives every hour's figure.
  let { title, hours, top }: { title: string; hours: number[]; top: number } = $props();
  const height = (value: number) => (top > 0 ? (value / top) * 96 : 0);
</script>

<figure>
  <figcaption>{title}</figcaption>
  <svg viewBox="0 0 240 100" preserveAspectRatio="none" aria-hidden="true">
    {#each hours as value, hour (hour)}
      <rect x={hour * 10 + 1} y={98 - height(value)} width="8" height={height(value)} />
    {/each}
    <line x1="0" y1="98.5" x2="240" y2="98.5" />
  </svg>
  <div class="axis" aria-hidden="true"><span>00:00</span><span>06:00</span><span>12:00</span><span>18:00</span></div>
</figure>

<style>
  figure { margin: 0; }
  figcaption { font-weight: 600; margin-bottom: 0.3rem; }
  svg { display: block; width: 100%; height: 9rem; }
  rect { fill: var(--accent); }
  line { stroke: var(--line); stroke-width: 1; vector-effect: non-scaling-stroke; }
  .axis { display: grid; grid-template-columns: repeat(4, 1fr); color: var(--muted); font-size: 0.85rem; }
</style>
