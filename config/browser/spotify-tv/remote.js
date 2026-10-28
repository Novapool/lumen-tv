// Lumen Spotify remote: why this exists.
// open.spotify.com is a mouse UI: its arrows only scroll, and its library rows,
// album/playlist cards and track rows are plain divs that react to clicks.
// Chromium's --enable-spatial-navigation was tried first (2026-10-27): it
// reached the header and player buttons but never stepped into Spotify's
// scrolling lists, and the first Down got stuck in the search box.
// So this script does the navigation itself:
// - the remote's arrows move focus to the nearest visible item in that
//   direction (buttons, links, rows, cards) and scroll it into view;
// - OK (Enter) on a card or library row opens it; on a track row it plays it
//   (Spotify plays on double-click); other buttons keep their own Enter;
// - sliders (progress, volume) are never focused: they'd take Left/Right as
//   seeks;
// - text boxes are skipped (they'd keep the arrows for their caret); a click
//   with the pad's touchpad still focuses them for typing;
// - remote.css draws a focus ring you can see from the sofa.
// Loaded with chromium --load-extension=<this dir> (see config/apps/youtube.toml
// for the DisableLoadExtensionCommandLineSwitch note).

const TEXT = 'input:not([type]), input[type="text"], input[type="search"], textarea, [contenteditable="true"]';
const OPEN = '[data-encore-id="card"], [aria-label="Your Library"] [role="row"]';
const PLAY = '[data-testid="tracklist-row"]';
const FOCUSABLE = `button, a[href], [role="button"], [role="link"], [role="tab"], [role="checkbox"], [tabindex="0"], ${OPEN}, ${PLAY}`;
const DIRS = { ArrowUp: [0, -1], ArrowDown: [0, 1], ArrowLeft: [-1, 0], ArrowRight: [1, 0] };

function setTabindex(el, value) {
  if (el.getAttribute("tabindex") !== value) el.setAttribute("tabindex", value);
}

function each(node, selector, fn) {
  if (node.matches(selector)) fn(node);
  for (const el of node.querySelectorAll(selector)) fn(el);
}

// Only added nodes are checked: Spotify's DOM changes constantly and the Pi is
// short on CPU.
function fix(node) {
  if (node.nodeType !== Node.ELEMENT_NODE) return;
  each(node, `${OPEN}, ${PLAY}`, (el) => setTabindex(el, "0"));
}

new MutationObserver((records) => {
  for (const r of records) for (const n of r.addedNodes) fix(n);
}).observe(document, { childList: true, subtree: true });

// The nearest ancestor that scrolls.
function scroller(el) {
  for (let p = el.parentElement; p; p = p.parentElement) {
    const o = getComputedStyle(p).overflowY;
    if ((o === "auto" || o === "scroll") && p.scrollHeight > p.clientHeight) return p;
  }
  return null;
}

// sameList: the candidate scrolls with the focused element, so it may sit just
// outside the visible part of the list (that is how Down scrolls a list).
function usable(el, r, sameList) {
  if (r.width < 4 || r.height < 4) return false;
  if (el.matches(TEXT) || el.closest('[aria-hidden="true"], [disabled], [inert]')) return false;
  if (sameList) return true;
  if (r.bottom < 0 || r.right < 0 || r.top > innerHeight || r.left > innerWidth) return false;
  // Covered by something else (a hidden scroller's off-screen part, a modal):
  // the point at its centre must belong to it.
  const hit = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
  return !!hit && (el === hit || el.contains(hit) || hit.contains(el));
}

// The nearest usable element whose centre lies in direction [dx, dy] from the
// focused one. The cross-axis offset costs 3x so moves stay in their row or
// column.
function next(from, [dx, dy]) {
  const a = from.getBoundingClientRect();
  const ax = a.left + a.width / 2, ay = a.top + a.height / 2;
  const list = scroller(from);
  let best = null, bestCost = Infinity;
  for (const el of document.querySelectorAll(FOCUSABLE)) {
    if (el === from || el.contains(from) || from.contains(el)) continue;
    const r = el.getBoundingClientRect();
    const along = dx ? (dx > 0 ? r.left - a.right : a.left - r.right) : (dy > 0 ? r.top - a.bottom : a.top - r.bottom);
    if (along < -4) continue;
    const bx = r.left + r.width / 2, by = r.top + r.height / 2;
    const cross = dx ? Math.abs(by - ay) : Math.abs(bx - ax);
    const cost = Math.max(along, 0) + 3 * cross;
    if (cost < bestCost && usable(el, r, !!list && list.contains(el))) { best = el; bestCost = cost; }
  }
  return best;
}

// Nothing focused yet: start at the player's play button.
function start() {
  return document.querySelector('[data-testid="control-button-playpause"]') || document.querySelector(FOCUSABLE);
}

document.addEventListener("keydown", (e) => {
  if (e.altKey || e.ctrlKey || e.metaKey) return;
  const el = document.activeElement;
  const here = el && el !== document.body ? el : null;
  if (here && here.matches(TEXT)) return;

  if (DIRS[e.key]) {
    // Arrows always move focus, even off a slider: Spotify's progress bar took
    // Left/Right as seeks (Laith, 2026-10-27). Seek and volume from the phone.
    const to = here ? next(here, DIRS[e.key]) : start();
    e.preventDefault();
    e.stopPropagation();
    if (to) {
      to.focus({ preventScroll: true });
      to.scrollIntoView({ block: "nearest", inline: "nearest" });
    }
    return;
  }

  if (e.key === "Enter" && here) {
    const opts = { bubbles: true, cancelable: true, view: window };
    if (here.matches(PLAY)) here.dispatchEvent(new MouseEvent("dblclick", opts));
    else if (here.matches(OPEN)) here.click();
    else return;
    e.preventDefault();
    e.stopPropagation();
  }
}, true);

document.documentElement.dataset.lumenRemote = "1";
