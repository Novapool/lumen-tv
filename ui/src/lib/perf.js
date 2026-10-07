// Frame-time sampler for the M1 perf gate. Records the gap between
// requestAnimationFrame callbacks; a 60 Hz display gives ~16.7 ms per frame.

export function createSampler() {
  let deltas = [];
  let last = 0;
  let raf = 0;

  function tick(t) {
    if (last) deltas.push(t - last);
    last = t;
    raf = requestAnimationFrame(tick);
  }

  return {
    start() {
      cancelAnimationFrame(raf);
      deltas = [];
      last = 0;
      raf = requestAnimationFrame(tick);
    },
    stop() {
      cancelAnimationFrame(raf);
      return summarize(deltas);
    },
    /** Stats for the most recent `n` frames, for the live overlay. */
    recent(n = 120) {
      return summarize(deltas.slice(-n));
    },
  };
}

export function summarize(deltas) {
  if (deltas.length === 0) return null;
  const total = deltas.reduce((a, b) => a + b, 0);
  const sorted = [...deltas].sort((a, b) => a - b);
  const round = (x) => Math.round(x * 10) / 10;
  return {
    frames: deltas.length,
    fps: round((1000 * deltas.length) / total),
    p95: round(sorted[Math.floor(0.95 * (sorted.length - 1))]),
    worst: round(sorted[sorted.length - 1]),
    slowPct: round((100 * deltas.filter((d) => d > 25).length) / deltas.length),
  };
}

/** The M1 gate from MILESTONES.md. */
export const passesGate = (s) => s && s.fps >= 55 && s.slowPct < 5;
