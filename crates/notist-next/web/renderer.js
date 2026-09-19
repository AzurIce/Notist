const native = { paragraph: 'p', section: 'section', strong: 'strong', em: 'em', 'list-item': 'li', terms: 'dl' };

export async function mount(parent, content, components, base = document.baseURI, attributes = {}) {
  parent.dataset.notistAttributes = JSON.stringify(attributes);
  document.documentElement.lang = typeof attributes.lang === 'string' ? attributes.lang : '';
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
    if ('link' in node) {
      const link = document.createElement('a');
      link.href = node.link;
      link.textContent = node.link;
      return link;
    }
    if (node.error) return failure(node.error, node);
    if (node.sequence) {
      const fragment = document.createDocumentFragment();
      for (const child of node.sequence) fragment.append(await render(child));
      return fragment;
    }
    try {
      const { item: name, args = {} } = node;
      const annotate = el => {
        if (node.attributes && Object.keys(node.attributes).length) {
          el.dataset.notistAttributes = JSON.stringify(node.attributes);
          if (typeof node.attributes.id === 'string') el.id = node.attributes.id;
        }
        return el;
      };
      if (name === 'linebreak') return annotate(document.createElement('br'));
      if (name === 'smartquote') {
        const lang = document.documentElement.lang.split('-')[0];
        const quotes = lang === 'de' ? ['\u201e','\u201c','\u201a','\u2018'] : lang === 'fr' ? ['\u00ab\u00a0','\u00a0\u00bb','\u2039','\u203a'] : ['\u201c','\u201d','\u2018','\u2019'];
        return document.createTextNode(quotes[(args.double ? 0 : 2) + (args.open ? 0 : 1)]);
      }
      if (name === 'raw' || name === 'math') {
        if (typeof args.content !== 'string') throw Error('Expected String content');
        const code = document.createElement('code');
        code.textContent = args.content;
        const el = args.block ? document.createElement('pre') : code;
        if (args.block) el.append(code);
        el.className = `notist-${name}`;
        if (args.lang) el.dataset.language = args.lang;
        return annotate(el);
      }
      if (name === 'link') {
        if (typeof args.dest !== 'string' || !['http:', 'https:', 'mailto:'].includes(new URL(args.dest, base).protocol)) throw Error('Unsupported link destination');
        const el = document.createElement('a');
        el.href = args.dest;
        el.append(await render(args.body));
        return annotate(el);
      }
      if (name === 'term-item') {
        const el = document.createElement('div'), term = document.createElement('dt'), description = document.createElement('dd');
        term.append(await render(args.term)); description.append(await render(args.body));
        el.append(term, description);
        return annotate(el);
      }
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
        annotate(el);
        if (name === 'list-item' && Number.isInteger(args.number)) el.value = args.number;
        if (typeof args.tight === 'boolean') el.dataset.tight = args.tight;
        if (name === 'section' && args.title) {
          const heading = document.createElement(`h${Math.max(1, Math.min(6, args.level || 1))}`);
          heading.append(await render(args.title));
          el.append(heading);
        }
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
      annotate(el);
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
