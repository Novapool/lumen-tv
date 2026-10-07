<script>
  // One app tile. Focus does one thing: the tile grows with its shadow, then a
  // sheen sweeps once. Only transform and opacity animate, and every animated
  // part keeps its own GPU layer (will-change) so WebKit never has to create or
  // drop layers mid-move — that showed up as a visible redraw on the Pi.
  let { app, focused = false, pressed = false } = $props();
</script>

<div class="tile" class:focused class:pressed>
  <div class="shadow"></div>
  <div class="face" style:background={app.bg}>
    <span class="glyph">{app.glyph}</span>
    <div class="gloss"></div>
    <div class="sheen"></div>
  </div>
  <div class="label">{app.name}</div>
</div>

<style>
  .tile {
    position: relative;
    flex: none;
    width: 19rem;
  }

  .face,
  .shadow {
    height: 11.4rem;
    border-radius: 1.1rem;
    transition: transform 240ms var(--ease-out), opacity 240ms var(--ease-out);
    will-change: transform, opacity;
  }

  .face {
    position: relative;
    overflow: hidden;
    display: grid;
    place-items: center;
  }

  .focused .face {
    transform: scale(1.12);
  }

  .pressed .face {
    transform: scale(1.05);
    transition-duration: 90ms;
  }

  /* The shadow is pre-painted on its own layer; focusing only fades it in. */
  .shadow {
    position: absolute;
    inset: 0;
    box-shadow: 0 2.2rem 3.2rem rgb(0 0 0 / 0.65);
    opacity: 0;
  }

  .focused .shadow {
    opacity: 1;
    transform: scale(1.12) translateY(0.5rem);
  }

  .glyph {
    font-size: 5rem;
    font-weight: 700;
    color: rgb(255 255 255 / 0.92);
    text-shadow: 0 0.2rem 0.6rem rgb(0 0 0 / 0.35);
  }

  .gloss {
    position: absolute;
    inset: 0;
    background: linear-gradient(to bottom, rgb(255 255 255 / 0.2), rgb(255 255 255 / 0.04) 45%, transparent 55%);
  }

  /* Starts after the grow finishes, so the two never compete. */
  .sheen {
    position: absolute;
    inset: 0;
    background: linear-gradient(105deg, transparent 35%, rgb(255 255 255 / 0.32) 50%, transparent 65%);
    transform: translateX(-120%);
    will-change: transform;
  }

  .focused .sheen {
    animation: sheen 800ms var(--ease-out) 260ms 1 both;
  }

  @keyframes sheen {
    to {
      transform: translateX(120%);
    }
  }

  .label {
    margin-top: 1.9rem;
    text-align: center;
    font-size: 1.5rem;
    font-weight: 600;
    opacity: 0;
    transition: opacity 200ms linear;
    will-change: opacity;
  }

  .focused .label {
    opacity: 1;
  }
</style>
