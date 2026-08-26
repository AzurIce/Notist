// Mermaid Web Component for the Notist mermaid plugin.
// The HTML renderer emits <notist-mermaid data-source="..." data-theme="...">;
// this module upgrades it into a real custom element that renders diagrams
// locally through the vendored @mermanjs/web-render package. Source validity
// is checked at evaluation time by semantic.wasm; the two stages intentionally
// have separate versioned parser contracts.

import { initMerman, renderSvgToElement } from './merman-web-render/dist/package-entries/render.js';

let rendererReady = null;

function loadRenderer() {
  if (!rendererReady) {
    rendererReady = initMerman();
  }
  return rendererReady;
}

class NotistMermaid extends HTMLElement {
  connectedCallback() {
    const source = this.dataset.source || '';
    const theme = this.dataset.theme || 'default';

    const shadow = this.attachShadow({ mode: 'open' });
    const style = document.createElement('style');
    style.textContent = `
      :host {
        display: block;
        margin: 1.25rem 0;
        color: #0f172a;
      }
      .notist-mermaid-frame {
        overflow-x: auto;
        padding: 1rem;
        border: 1px solid #e2e8f0;
        border-radius: 10px;
        background: #ffffff;
        box-shadow: 0 1px 2px rgb(15 23 42 / 0.06);
        scrollbar-color: #94a3b8 transparent;
      }
      .notist-mermaid-figure {
        min-width: min(100%, 42rem);
      }
      .notist-mermaid-figure svg {
        display: block;
        width: max-content;
        min-width: 100%;
        max-width: none;
        height: auto;
        margin: 0 auto;
      }
      slot {
        display: block;
        margin-top: 0.5rem;
        color: rgb(0 0 0 / 0.65);
      }
      pre {
        overflow: auto;
        padding: 0.75rem;
        border: 1px solid #e2e8f0;
        border-radius: 8px;
        background: #f8fafc;
      }
      :host([data-theme="dark"]) {
        color: #e2e8f0;
      }
      :host([data-theme="dark"]) .notist-mermaid-frame,
      :host([data-theme="dark"]) pre {
        border-color: #334155;
        background: #0f172a;
        box-shadow: none;
      }
      :host([data-theme="neutral"]) .notist-mermaid-frame {
        box-shadow: none;
      }
    `;
    shadow.appendChild(style);
    if (this.childNodes.length) {
      shadow.appendChild(document.createElement('slot'));
    }

    if (!source) {
      this._showFallback(shadow, 'Empty mermaid diagram.');
      return;
    }

    loadRenderer()
      .then(() => {
        const figure = document.createElement('div');
        figure.className = 'notist-mermaid-figure';
        const normalizedTheme = theme.trim().toLowerCase() || 'default';
        renderSvgToElement(figure, source, JSON.stringify({
          site_config: { theme: normalizedTheme },
        }));
        const frame = document.createElement('div');
        frame.className = 'notist-mermaid-frame';
        frame.appendChild(figure);
        shadow.appendChild(frame);
      })
      .catch((error) => {
        console.error('notist-mermaid render failed', error);
        this._showFallback(shadow, 'Mermaid rendering failed.');
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

customElements.define('notist-mermaid', NotistMermaid);
