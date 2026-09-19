// Resource transport fixture: displays source, without creating a graphics context.
export default class Shader extends HTMLElement {
  render(args) {
    if (typeof args.source !== 'string') throw new Error('shader.source must be String');
    const pre = document.createElement('pre');
    pre.textContent = args.source;
    this.replaceChildren(pre);
  }
}
