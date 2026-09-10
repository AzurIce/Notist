// Math Web Component for the Notist math plugin.
// The HTML renderer claims `core::math` and emits
// <notist-math data-source="..." data-block="true|false">; this module
// compiles the Typst math source in-browser through the vendored typst.ts
// (compiler + renderer wasm, New Computer Modern fonts) and mounts the
// resulting SVG into a shadow root. Without the wasm assets the component
// degrades to a <pre> with the raw source, mirroring the mermaid plugin's
// fallback. Sources are not validated at evaluation time; broken math
// surfaces as the same fallback in preview.
//
// The engine resolves its wasm/font bytes through the host-provided asset
// map (`globalThis.__NOTIST_ASSET_URLS__`, site-root-relative) when running
// as a blob module inside sandboxed preview iframes; on a plain site it
// falls back to URLs relative to this file.

const ASSETS = globalThis.__NOTIST_ASSET_URLS__ || null;

const FONTS = [
  'fonts/NewCMMath-Regular.otf',
  'fonts/NewCMMath-Bold.otf',
  'fonts/NewCM10-Regular.otf',
  'fonts/NewCM10-Italic.otf',
  'fonts/LibertinusSerif-Regular.otf',
  'fonts/LibertinusSerif-Italic.otf',
  'fonts/DejaVuSansMono.ttf',
];

function assetUrl(relative) {
  const key = `plugins/math/${relative}`;
  if (ASSETS && ASSETS[key]) return ASSETS[key];
  return new URL(relative, import.meta.url).href;
}

async function fetchBytes(relative) {
  const response = await fetch(assetUrl(relative));
  return new Uint8Array(await response.arrayBuffer());
}

let engineReady = null;

function loadEngine() {
  if (!engineReady) engineReady = initEngine();
  return engineReady;
}

async function initEngine() {
  try {
    return await initEngineInner();
  } catch (error) {
    // Surfaced for sandboxed environments (preview iframes) where the
    // module's own console is hard to reach.
    globalThis.__NOTIST_MATH_ERROR__ = String((error && error.stack) || error);
    throw error;
  }
}

async function initEngineInner() {
  const [
    { $typst },
    { disableDefaultFontAssets, loadFonts },
    { createTypstCompiler },
    { createTypstRenderer },
  ] = await Promise.all([
    import('./vendor/typst-ts/dist/esm/contrib/snippet.mjs'),
    import('./vendor/typst-ts/dist/esm/options.init.mjs'),
    import('./vendor/typst-ts/dist/esm/compiler.mjs'),
    import('./vendor/typst-ts/dist/esm/renderer.mjs'),
  ]);
  const compilerGlue = await import('./vendor/typst-ts/pkg/typst_ts_web_compiler.mjs');
  const rendererGlue = await import('./vendor/typst-ts/pkg/typst_ts_renderer.mjs');

  // Both components are injected explicitly: the snippet's default
  // bootstraps import bare `@myriaddreamin/typst.ts/...` specifiers, which
  // cannot resolve outside a bundler. The wasm bytes are handed over
  // explicitly too — the glue's import.meta.url resolution dies inside a
  // blob module and is needlessly network-bound on a static site.
  const fontBytes = await Promise.all(FONTS.map((font) => fetchBytes(font)));
  const compiler = createTypstCompiler();
  await compiler.init({
    getModule: async () => fetchBytes('vendor/typst-ts/pkg/typst_ts_web_compiler_bg.wasm'),
    getWrapper: async () => compilerGlue,
    beforeBuild: [
      disableDefaultFontAssets(),
      // The library's default fetcher pulls a node-only cache package;
      // plain fetch covers everything since fonts arrive as bytes.
      loadFonts(fontBytes, { assets: false, fetcher: (url) => fetch(url) }),
    ],
  });
  $typst.setCompiler(Promise.resolve(compiler));

  const renderer = createTypstRenderer();
  await renderer.init({
    getModule: async () => fetchBytes('vendor/typst-ts/pkg/typst_ts_renderer_bg.wasm'),
    getWrapper: async () => rendererGlue,
  });
  $typst.setRenderer(Promise.resolve(renderer));
  return $typst;
}

async function renderSvg(source, block) {
  try {
    const $typst = await loadEngine();
    // Typst marks display math by spaces just inside the delimiters.
    const equation = block ? `$ ${source} $` : `$${source}$`;
    const mainContent = [
      '#set page(width: auto, height: auto, margin: 4pt)',
      '#set text(size: 15pt)',
      equation,
      '',
    ].join('\n');
    return await $typst.svg({ mainContent });
  } catch (error) {
    globalThis.__NOTIST_MATH_ERROR__ = String((error && error.stack) || error);
    throw error;
  }
}

const svgCache = new Map();

class NotistMath extends HTMLElement {
  connectedCallback() {
    const source = this.dataset.source || '';
    const block = this.dataset.block === 'true';

    const shadow = this.attachShadow({ mode: 'open' });
    const style = document.createElement('style');
    style.textContent = `
      :host {
        display: block;
        margin: 0.75rem 0;
        color: #0f172a;
      }
      :host(:not([data-block="true"])) {
        display: inline;
        margin: 0;
      }
      .notist-math-figure svg {
        display: block;
        width: max-content;
        max-width: 100%;
        height: auto;
        margin: 0 auto;
      }
      /* Inline math rides the text line: the figure must not break the
       * paragraph flow, and middle alignment approximates the math axis
       * (the SVG carries no baseline). */
      :host(:not([data-block="true"])) .notist-math-figure {
        display: inline-block;
      }
      :host(:not([data-block="true"])) .notist-math-figure svg {
        display: inline-block;
        margin: 0;
        vertical-align: middle;
      }
      pre {
        overflow: auto;
        padding: 0.75rem;
        border: 1px solid #e2e8f0;
        border-radius: 8px;
        background: #f8fafc;
      }
      :host([data-block="true"]) pre {
        border-color: #334155;
        background: #0f172a;
        color: #e2e8f0;
      }
    `;
    shadow.appendChild(style);

    if (!source) {
      this._showFallback(shadow, 'Empty math source.');
      return;
    }

    const cacheKey = `${block ? 'block' : 'inline'}\u0000${source}`;
    let svgPromise = svgCache.get(cacheKey);
    if (!svgPromise) {
      svgPromise = renderSvg(source, block);
      svgCache.set(cacheKey, svgPromise);
      svgPromise.catch(() => svgCache.delete(cacheKey));
    }
    svgPromise
      .then((svg) => {
        const figure = document.createElement('div');
        figure.className = 'notist-math-figure';
        figure.innerHTML = svg;
        shadow.appendChild(figure);
      })
      .catch((error) => {
        console.error('notist-math render failed', error);
        this._showFallback(shadow, 'Math rendering failed.');
      });
  }

  _showFallback(shadow, message) {
    const note = document.createElement('p');
    note.textContent = message;
    shadow.appendChild(note);
    const fallback = document.createElement('pre');
    const code = document.createElement('code');
    code.textContent = this.dataset.source || '';
    fallback.appendChild(code);
    shadow.appendChild(fallback);
  }
}

customElements.define('notist-math', NotistMath);
