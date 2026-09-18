const native = { paragraph: 'p', section: 'section', strong: 'strong', em: 'em', 'list-item': 'li' };

export async function mount(parent, content, components, base = document.baseURI) {
  const loaded = new Map();
  const failure = (message, node = {}) => {
    const el = document.createElement('notist-error');
    el.setAttribute('role', 'note'); el.textContent = message;
    el.dataset.source = node.source ?? ''; el.dataset.offset = node.offset ?? 0;
    return el;
  };
  const render = async node => {
    if (node == null) return document.createDocumentFragment();
    if (typeof node !== 'object') throw new Error('Expected Content');
    if ('text' in node) return document.createTextNode(node.text);
    if (node.error) return failure(node.error, node);
    if (node.sequence) {
      const fragment = document.createDocumentFragment();
      for (const child of node.sequence) fragment.append(await render(child));
      return fragment;
    }
    try {
      const { item: name, args = {} } = node;
      let tag = native[name];
      if (name === 'list') tag = args.ordered === true ? 'ol' : 'ul';
      if (name === 'heading') {
        const level = args.level ?? 2;
        if (!Number.isInteger(level) || level < 1 || level > 6) throw new Error('Invalid heading level');
        tag = `h${level}`;
      }
      if (tag) {
        if (!('body' in args)) throw new Error(`Missing body for ${name}`);
        const el = document.createElement(tag);
        const children = Array.isArray(args.body) ? args.body : [args.body];
        for (const child of children) el.append(await render(child));
        return el;
      }
      if (!/^[a-z0-9-]+$/.test(name)) throw new Error(`Invalid component name: ${name}`);
      if (!components[name]) throw new Error(`Missing component: ${name}`);
      if (!loaded.has(name)) loaded.set(name, import(new URL(components[name], base).href));
      const implementation = await loaded.get(name);
      tag = `notist-${name}`;
      if (!customElements.get(tag)) customElements.define(tag, implementation.default);
      else if (customElements.get(tag) !== implementation.default) throw new Error(`Component conflict: ${name}`);
      const el = document.createElement(tag);
      for (const [key, value] of Object.entries(args)) {
        if (!/^[a-z0-9-]+$/.test(key)) throw new Error(`Invalid component field: ${key}`);
        el.setAttribute(`data-${key}`, JSON.stringify(value));
      }
      if (typeof el.render !== 'function') throw new Error(`Component ${name} must implement render(args, context)`);
      await el.render(args, { renderContent: render });
      return el;
    } catch (error) { return failure(error.message, node); }
  };
  parent.replaceChildren(await render(content));
}
