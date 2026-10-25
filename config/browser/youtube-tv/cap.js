// Lumen YouTube cap: why this exists.
// The Pi 5 decodes video in software. It can't do AV1 and can't keep up above
// 1080p, but YouTube picks AV1 and 4K whenever the browser claims support. This
// content script (MAIN world, document_start) wraps the browser's capability
// probes so those formats read as unsupported; avc1 (H.264) and vp09/vp9 pass.
// Loaded with chromium --load-extension=<this dir>. Chromium 137+ only honours
// that switch with --disable-features=DisableLoadExtensionCommandLineSwitch
// (unverified for Chromium 155 on Debian; Pi check V5 verifies it via the
// data-lumen-cap marker set below).

const MAX_WIDTH = 1920;
const MAX_HEIGHT = 1080;

// Does a codec string / MIME string mention AV1? ("av01.0.08M.08")
function hasAv1(mime) {
  return /\bav01\b/i.test(String(mime || ""));
}

// Pull width/height out of MIME parameters: 'video/mp4; codecs="..."; width=3840'.
// Returns 0 for a missing value.
function mimeSize(mime) {
  const s = String(mime || "");
  const w = /(?:^|[;\s])width\s*=\s*"?(\d+)/i.exec(s);
  const h = /(?:^|[;\s])height\s*=\s*"?(\d+)/i.exec(s);
  return { width: w ? +w[1] : 0, height: h ? +h[1] : 0 };
}

// True when this MIME string must read as unsupported.
function mimeRejected(mime) {
  if (hasAv1(mime)) return true;
  const { width, height } = mimeSize(mime);
  return width > MAX_WIDTH || height > MAX_HEIGHT;
}

// True when a mediaCapabilities decodingInfo config must read as unsupported.
function configRejected(cfg) {
  const v = cfg && cfg.video;
  if (!v) return false;
  if (mimeRejected(v.contentType)) return true;
  return (+v.width || 0) > MAX_WIDTH || (+v.height || 0) > MAX_HEIGHT;
}

function install() {
  // MediaSource / ManagedMediaSource .isTypeSupported(mime) -> false
  for (const name of ["MediaSource", "ManagedMediaSource"]) {
    const C = window[name];
    if (!C || typeof C.isTypeSupported !== "function") continue;
    const orig = C.isTypeSupported.bind(C);
    C.isTypeSupported = (mime) => (mimeRejected(mime) ? false : orig(mime));
  }

  // HTMLMediaElement.canPlayType(mime) -> "" (the "no" answer)
  const origCan = HTMLMediaElement.prototype.canPlayType;
  HTMLMediaElement.prototype.canPlayType = function (mime) {
    return mimeRejected(mime) ? "" : origCan.call(this, mime);
  };

  // navigator.mediaCapabilities.decodingInfo(cfg) -> unsupported result
  const mc = navigator.mediaCapabilities;
  if (mc && typeof mc.decodingInfo === "function") {
    const origInfo = mc.decodingInfo.bind(mc);
    mc.decodingInfo = (cfg) =>
      configRejected(cfg)
        ? Promise.resolve({ supported: false, smooth: false, powerEfficient: false })
        : origInfo(cfg);
  }
}

// Browser only: skipped when loaded by node for the hand tests.
if (typeof window !== "undefined" && typeof document !== "undefined") {
  install();
  // Marker so the Pi check can prove the extension loaded. At document_start
  // the root may not exist yet, so wait for it if needed.
  const mark = () => {
    if (!document.documentElement) return false;
    document.documentElement.dataset.lumenCap = "1";
    return true;
  };
  if (!mark()) {
    const obs = new MutationObserver(() => mark() && obs.disconnect());
    obs.observe(document, { childList: true });
  }
} else if (typeof module !== "undefined") {
  module.exports = { hasAv1, mimeSize, mimeRejected, configRejected };
}
