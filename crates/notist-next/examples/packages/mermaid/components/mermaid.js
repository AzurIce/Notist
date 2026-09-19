// Resource transport fixture: displays source, without a bundled Mermaid engine.
export default class Mermaid extends HTMLElement {
  render(args) {
    if (typeof args.source !== 'string') throw new Error('mermaid.source must be String');
    const pre = document.createElement('pre');
    pre.textContent = args.source;
    this.replaceChildren(pre);
  }
}
