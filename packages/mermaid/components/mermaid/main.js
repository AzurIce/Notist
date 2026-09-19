const ready = new Promise((resolve, reject) => {
  const script = document.createElement('script');
  script.src = new URL('./mermaid.min.js', import.meta.url).href;
  script.onload = resolve;
  script.onerror = () => reject(Error('Unable to load Mermaid'));
  document.head.append(script);
});
let next = 0;
let queue = Promise.resolve();
export default class Mermaid extends HTMLElement {
  async render(args) {
    if (typeof args.source !== 'string') throw Error('mermaid.source must be String');
    const task = queue.then(async () => {
      await ready;
      globalThis.mermaid.initialize({ startOnLoad: false, securityLevel: 'strict', theme: args.theme || 'default' });
      const { svg } = await globalThis.mermaid.render(`notist-mermaid-${next++}`, args.source);
      this.innerHTML = svg;
    });
    queue = task.catch(() => {});
    await task;
  }
}
