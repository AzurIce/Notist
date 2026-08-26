// Mermaid Web Component for the Notist mermaid plugin.
// The HTML renderer emits <notist-mermaid data-source="..." data-theme="...">;
// this module upgrades it into a real custom element that renders the diagram
// in the browser with Mermaid (ESM from CDN). Source validity was already
// checked at evaluation time by semantic.wasm.

let mermaidPromise = null;

function loadMermaid() {
  if (!mermaidPromise) {
    mermaidPromise = import(
      'https://cdn.jsdelivr.net/npm/mermaid@11/dist/mermaid.esm.min.mjs'
    );
  }
  return mermaidPromise;
}

let nextDiagramId = 0;

class NotistMermaid extends HTMLElement {
  connectedCallback() {
    const source = this.dataset.source || '';
    const theme = this.dataset.theme || 'default';

    const shadow = this.attachShadow({ mode: 'open' });
    const style = document.createElement('style');
    style.textContent = `
      :host {
        display: block;
        margin: 1rem 0;
      }
      .notist-mermaid-figure svg {
        max-width: 100%;
        height: auto;
      }
      slot {
        display: block;
        margin-top: 0.5rem;
        color: rgb(0 0 0 / 0.65);
      }
      pre {
        overflow: auto;
        padding: 0.75rem;
        border-radius: 8px;
        background: rgb(0 0 0 / 0.04);
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

    loadMermaid()
      .then(async ({ default: mermaid }) => {
        mermaid.initialize({ startOnLoad: false, theme });
        const id = `notist-mermaid-${nextDiagramId++}`;
        const { svg } = await mermaid.render(id, source);
        const figure = document.createElement('div');
        figure.className = 'notist-mermaid-figure';
        figure.innerHTML = svg;
        shadow.appendChild(figure);
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
