<script>
  // One app tile. Focus grows the face, fades in its pre-painted shadow and
  // fades in the name. All of it uses "hold" animations (see app.css): the
  // change plays in the first 240 ms and the end value then holds, so WebKit
  // never shows a stale value when an animation ends.
  // `near` = within one tile of focus. Tiles further away drop their
  // animations: by then the shrink has finished, and removing the animation
  // from the main thread commits the final value in the same frame.
  let { app, focused = false, near = false, pressed = false } = $props();

  let left = $state(false); // has been focused, so it plays the shrink
  let released = $state(false); // has been pressed, so it plays the release
  $effect(() => {
    if (focused) left = true;
    else if (!near) left = false;
  });
  $effect(() => {
    if (pressed) released = true;
    else if (!focused) released = false;
  });
</script>

<div class="tile" class:focused class:left class:pressed class:released>
  <div class="shadow"></div>
  <div class="face" style:background={app.bg}>
    <span class="glyph">{app.glyph}</span>
    <div class="gloss"></div>
  </div>
  <div class="label">{app.name}</div>
</div>

<style>
  .tile {
    position: relative;
    flex: none;
    width: 19rem;
  }

  .tile,
  .face,
  .shadow,
  .label {
    will-change: transform, opacity;
  }

  .face,
  .shadow {
    height: 11.4rem;
    border-radius: 1.1rem;
  }

  .face {
    position: relative;
    overflow: hidden;
    display: grid;
    place-items: center;
  }

  /* The shadow is pre-painted on its own layer; focusing only fades it in. */
  .shadow {
    position: absolute;
    inset: 0;
    box-shadow: 0 2.2rem 3.2rem rgb(0 0 0 / 0.65);
    opacity: 0;
  }

  .label {
    margin-top: 1.9rem;
    text-align: center;
    font-size: 1.5rem;
    font-weight: 600;
    opacity: 0;
  }

  .left .face {
    animation: face-out var(--hold) var(--ease-out) both;
  }

  .left .shadow {
    animation: shadow-out var(--hold) var(--ease-out) both;
  }

  .left .label {
    animation: label-out var(--hold) linear both;
  }

  /* End values also set as plain styles, so they hold once an animation is dropped. */
  .focused .face {
    transform: scale(1.12);
    animation: face-in var(--hold) var(--ease-out) both;
  }

  .focused .shadow {
    opacity: 1;
    transform: scale(1.12) translateY(0.5rem);
    animation: shadow-in var(--hold) var(--ease-out) both;
  }

  .focused .label {
    opacity: 1;
    animation: label-in var(--hold) linear both;
  }

  /* Press shrinks the whole tile (face 1.12 -> 1.05) and springs back. */
  .released {
    animation: release var(--hold) var(--ease-out) both;
  }

  .pressed {
    animation: press var(--hold) var(--ease-out) both;
  }

  /* --hold is 1000 s: 0.024% = 240 ms, 0.02% = 200 ms, 0.009% = 90 ms. */
  @keyframes face-in {
    0% { transform: scale(1); }
    0.024%, 100% { transform: scale(1.12); }
  }

  @keyframes face-out {
    0% { transform: scale(1.12); }
    0.024%, 100% { transform: scale(1); }
  }

  @keyframes shadow-in {
    0% { opacity: 0; transform: scale(1) translateY(0); }
    0.024%, 100% { opacity: 1; transform: scale(1.12) translateY(0.5rem); }
  }

  @keyframes shadow-out {
    0% { opacity: 1; transform: scale(1.12) translateY(0.5rem); }
    0.024%, 100% { opacity: 0; transform: scale(1) translateY(0); }
  }

  @keyframes label-in {
    0% { opacity: 0; }
    0.02%, 100% { opacity: 1; }
  }

  @keyframes label-out {
    0% { opacity: 1; }
    0.02%, 100% { opacity: 0; }
  }

  @keyframes press {
    0% { transform: scale(1); }
    0.009%, 100% { transform: scale(0.9375); }
  }

  @keyframes release {
    0% { transform: scale(0.9375); }
    0.024%, 100% { transform: scale(1); }
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
</style>
