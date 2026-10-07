<script>
  // M1 home screen: one row of static tiles, a "top shelf" backdrop that
  // cross-fades to the focused app, keyboard navigation and a frame-time
  // overlay. M2 replaces the keydown handler with backend nav events.
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import Tile from './lib/Tile.svelte';
  import { placeholderApps as apps } from './lib/placeholder-apps.js';
  import { createSampler, passesGate } from './lib/perf.js';

  const STEP_REM = 22; // tile width (19) + gap (3)
  const VISIBLE = 5;

  let focused = $state(0);
  let pressed = $state(false);
  let rowOffset = $derived(Math.max(0, Math.min(focused - 1, apps.length - VISIBLE)) * STEP_REM);


  // The row scrolls with a Web Animation in the same "hold" style as the CSS
  // ones (see app.css): it moves in 320 ms, then holds the end value, and the
  // next move replaces it in the same frame.
  const HOLD_MS = 1e6;
  let rowEl;
  let rowAnim;
  $effect(() => {
    const to = `translateX(-${rowOffset}rem)`;
    const from = getComputedStyle(rowEl).transform; // includes the running animation
    const next = rowEl.animate(
      [
        { transform: from, easing: 'cubic-bezier(0.2, 0.8, 0.2, 1)' },
        { transform: to, offset: 320 / HOLD_MS },
        { transform: to },
      ],
      { duration: HOLD_MS, fill: 'forwards' },
    );
    rowAnim?.cancel();
    rowAnim = next;
  });

  // Two backdrop layers: the hidden one gets the new colour, then they swap
  // opacity. (One pre-painted layer per app was slower on the Pi: 12 full-width layers.)
  let heroBg = $state([apps[0].bg, apps[0].bg]);
  let heroFront = $state(0);

  let clock = $state(timeNow());
  let showOverlay = $state(false);
  let live = $state(null);
  let bench = $state(null); // { phase, secondsLeft } or { result }

  const sampler = createSampler(); // benchmark
  const liveSampler = createSampler(); // overlay, kept separate so its cleanup can't stop a benchmark

  function timeNow() {
    return new Date().toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' });
  }

  function move(dir) {
    const next = focused + dir;
    if (next < 0 || next >= apps.length) return false;
    focused = next;
    const back = 1 - heroFront;
    heroBg[back] = apps[next].bg;
    heroFront = back;
    return true;
  }

  function press() {
    pressed = true;
    setTimeout(() => (pressed = false), 140);
  }

  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

  // Simulates holding Right (then Left at the ends) at a typical key-repeat
  // rate, after a short idle baseline so a 30 Hz TV mode is easy to spot.
  async function runBench() {
    if (bench?.phase) return;
    showOverlay = true;
    bench = { phase: 'idle baseline', secondsLeft: 3 };
    sampler.start();
    await sleep(3000);
    const idle = sampler.stop();

    sampler.start();
    let dir = 1;
    const end = performance.now() + 30000;
    while (performance.now() < end) {
      if (!move(dir)) {
        dir = -dir;
        move(dir);
      }
      bench = { phase: 'holding Right', secondsLeft: Math.ceil((end - performance.now()) / 1000) };
      await sleep(150);
    }
    const motion = sampler.stop();

    const result = {
      idle,
      motion,
      pass: passesGate(motion),
      viewport: `${innerWidth}x${innerHeight}@${devicePixelRatio}x`,
      userAgent: navigator.userAgent,
    };
    bench = { result };
    invoke('perf_report', { report: JSON.stringify(result) }).catch(() => {});
  }

  function onKey(e) {
    if (e.ctrlKey && e.key.toLowerCase() === 'q') return invoke('quit').catch(() => {});
    if (bench?.phase) return;
    switch (e.key) {
      case 'ArrowRight': move(1); break;
      case 'ArrowLeft': move(-1); break;
      case 'Enter': case ' ': press(); break;
      case 'F2': case 'i': showOverlay = !showOverlay; break;
      case 'b': runBench(); break;
      default: return;
    }
    e.preventDefault();
  }

  // The live overlay keeps the sampler running only while it is visible.
  $effect(() => {
    if (!showOverlay || bench?.phase) return;
    liveSampler.start();
    const id = setInterval(() => (live = liveSampler.recent(120)), 500);
    return () => {
      clearInterval(id);
      liveSampler.stop();
    };
  });

  onMount(() => {
    const id = setInterval(() => (clock = timeNow()), 10000);
    invoke('bench_requested')
      .then((yes) => yes && setTimeout(runBench, 3000))
      .catch(() => {});
    return () => clearInterval(id);
  });
</script>

<svelte:window onkeydown={onKey} />

<main>
  <div class="hero" aria-hidden="true">
    {#each heroBg as bg, i}
      <div class="hero-layer" class:front={heroFront === i} style:background={bg}></div>
    {/each}
    <div class="hero-fade"></div>
  </div>

  <header>
    <div class="wordmark">Lumen</div>
    <div class="clock">{clock}</div>
  </header>

  <section class="shelf">
    <h1 class="title">{apps[focused].name}</h1>
  </section>

  <div class="row-viewport">
    <div class="row" bind:this={rowEl} style:transform="translateX(-{rowOffset}rem)">
      {#each apps as app, i (app.id)}
        <Tile {app} focused={i === focused} near={Math.abs(i - focused) <= 1} pressed={pressed && i === focused} />
      {/each}
    </div>
  </div>

  {#if showOverlay}
    <aside class="perf">
      {#if bench?.phase}
        <div>Benchmark: {bench.phase}… {bench.secondsLeft}s</div>
      {:else if bench?.result}
        {@const m = bench.result.motion}
        <div class:pass={bench.result.pass} class:fail={!bench.result.pass}>
          {bench.result.pass ? 'PASS' : 'FAIL'} · {m.fps} fps · {m.slowPct}% slow
        </div>
        <div>p95 {m.p95} ms · worst {m.worst} ms</div>
        <div>idle {bench.result.idle?.fps} fps · {bench.result.viewport}</div>
      {:else if live}
        <div>{live.fps} fps · p95 {live.p95} ms · {live.slowPct}% slow</div>
      {:else}
        <div>measuring…</div>
      {/if}
      <div class="hint">B benchmark · F2 hide · Ctrl+Q quit</div>
    </aside>
  {/if}
</main>

<style>
  main {
    position: relative;
    height: 100%;
    overflow: hidden;
  }

  .hero {
    position: absolute;
    inset: 0 0 35% 0;
  }

  .hero-layer {
    position: absolute;
    inset: 0;
    opacity: 0;
    animation: hero-out var(--hold) ease both;
    will-change: opacity;
  }

  .hero-layer.front {
    opacity: 0.55;
    animation-name: hero-in;
  }

  /* 600 ms of the 1000 s hold = 0.06% */
  @keyframes hero-in {
    0% { opacity: 0; }
    0.06%, 100% { opacity: 0.55; }
  }

  @keyframes hero-out {
    0% { opacity: 0.55; }
    0.06%, 100% { opacity: 0; }
  }

  .hero-fade {
    position: absolute;
    inset: 0;
    background:
      radial-gradient(ellipse at 70% 0%, transparent 30%, var(--bg) 85%),
      linear-gradient(to bottom, transparent 40%, var(--bg));
  }

  header {
    position: absolute;
    top: var(--safe-y);
    left: var(--safe-x);
    right: var(--safe-x);
    display: flex;
    justify-content: space-between;
    align-items: baseline;
  }

  .wordmark {
    font-size: 2rem;
    font-weight: 750;
    letter-spacing: 0.02em;
  }

  .clock {
    font-size: 1.9rem;
    font-weight: 500;
    color: var(--muted);
    font-variant-numeric: tabular-nums;
  }

  .shelf {
    position: absolute;
    left: var(--safe-x);
    top: 14rem;
  }

  .title {
    margin: 0;
    font-size: 5.2rem;
    font-weight: 750;
    letter-spacing: -0.02em;
  }

  .row-viewport {
    position: absolute;
    left: var(--safe-x);
    right: 0;
    top: 33rem;
    padding: 2rem 0 5rem; /* room for the focused tile's scale + shadow */
  }

  .row {
    display: flex;
    gap: 3rem;
    will-change: transform;
  }

  .perf {
    position: absolute;
    top: var(--safe-y);
    right: var(--safe-x);
    margin-top: 3.5rem;
    padding: 1rem 1.4rem;
    border-radius: 0.8rem;
    background: rgb(0 0 0 / 0.7);
    font: 500 1.15rem/1.6 ui-monospace, 'DejaVu Sans Mono', monospace;
    text-align: right;
  }

  .pass {
    color: #5ee28a;
  }

  .fail {
    color: #ff6b6b;
  }

  .hint {
    color: var(--muted);
    font-size: 0.95rem;
  }
</style>
