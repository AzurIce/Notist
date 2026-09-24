const native = { paragraph: 'p', section: 'section', strong: 'strong', em: 'em', underline: 'u', strike: 's', 'list-item': 'li', terms: 'dl' };

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
    try {
      const { item: name, args = {} } = node;
      const annotate = (el, source = node) => {
        if (source.attributes && Object.keys(source.attributes).length) {
          el.dataset.notistAttributes = JSON.stringify(source.attributes);

        }
        if (typeof source.label === 'string') el.dataset.notistLabel = source.label;
        return el;
      };
      if (name === 'seq') {
        const children = args.children ?? [];
        const fragment = document.createDocumentFragment();
        let group = null, groupKind = null;
        for (const child of children) {
          const kind = child.item === 'term-item' ? 'dl' : child.item === 'list-item' ? (child.args?.ordered ? 'ol' : 'ul') : null;
          if (kind) {
            if (kind !== groupKind) {
              group = document.createElement(kind); groupKind = kind;
              if (kind === 'ol' && Number.isInteger(child.args?.number)) group.start = child.args.number;
              fragment.append(group);
            }
            group.append(await render(child));
          } else { group = null; groupKind = null; fragment.append(await render(child)); }
        }
        if (!Object.keys(node.attributes ?? {}).length && node.label == null) return fragment;
        const wrapper = annotate(document.createElement('notist-seq'));
        wrapper.style.display = 'contents'; wrapper.append(fragment); return wrapper;
      }
      if (name === 'text' || name === 'space') {
        const text = document.createTextNode(name === 'space' ? ' ' : args.text ?? '');
        if (!Object.keys(node.attributes ?? {}).length && node.label == null) return text;
        const wrapper = annotate(document.createElement('span')); wrapper.append(text); return wrapper;
      }
      if (name === 'error') return annotate(failure(args.message ?? 'Invalid error Item', node));
      if (name === 'linebreak') return annotate(document.createElement('br'));
      if (name === 'smartquote') {
        const lang = document.documentElement.lang.split('-')[0];
        const quotes = lang === 'de' ? ['\u201e','\u201c','\u201a','\u2018'] : lang === 'fr' ? ['\u00ab\u00a0','\u00a0\u00bb','\u2039','\u203a'] : ['\u201c','\u201d','\u2018','\u2019'];
        const text = document.createTextNode(quotes[(args.double ? 0 : 2) + (args.open ? 0 : 1)]);
        if (!Object.keys(node.attributes ?? {}).length && node.label == null) return text;
        const wrapper = annotate(document.createElement('span')); wrapper.append(text); return wrapper;
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
        if (args.target) {
          const target = args.target.target + (args.target.labels ?? []).map(label => '::' + JSON.stringify(label)).join('');
          const el = annotate(document.createElement('a')); el.href = target; el.textContent = target; return el;
        }

        if (typeof args.dest !== 'string' || !['http:', 'https:', 'mailto:'].includes(new URL(args.dest, base).protocol)) throw Error('Unsupported link destination');
        const el = document.createElement('a');
        el.href = args.dest;
        el.append(await render(args.body));
        return annotate(el);
      }
      if (name === 'rule') return annotate(document.createElement('hr'));
      if (name === 'image') {
        if (typeof args.source !== 'string' || !args.source || typeof args.alt !== 'string') throw Error('Image requires source and alt');
        const url = new URL(args.source, base);
        if (!['http:', 'https:'].includes(url.protocol)) throw Error('Unsupported image source');
        const el = document.createElement('img');
        el.src = args.source; el.alt = args.alt;
        if (args.block === true) el.style.display = 'block';
        return annotate(el);
      }
      if (['callout', 'details', 'quote', 'figure'].includes(name)) {
        if (!args.body || typeof args.body !== 'object') throw Error(`Missing body for ${name}`);
        const tag = { callout: 'aside', details: 'details', quote: 'blockquote', figure: 'figure' }[name];
        const el = annotate(document.createElement(tag));
        if (name === 'callout') {
          el.className = 'notist-callout'; el.dataset.kind = args.kind ?? 'note';
          if (args.title) { const title = document.createElement('header'); title.append(await render(args.title)); el.append(title); }
        }
        if (name === 'details') {
          el.open = args.open === true;
          if (args.summary) { const summary = document.createElement('summary'); summary.append(await render(args.summary)); el.append(summary); }
        }
        if (name === 'figure') el.dataset.kind = args.kind ?? 'figure';
        el.append(await render(args.body));
        if (name === 'quote' && args.attribution) { const footer = document.createElement('footer'); footer.append(await render(args.attribution)); el.append(footer); }
        if (name === 'figure' && args.caption) { const caption = document.createElement('figcaption'); caption.append(await render(args.caption)); el.append(caption); }
        return el;
      }
      if (name === 'table-cell') {
        const el = annotate(document.createElement('td'));
        if (!args.body) throw Error('Missing table-cell body');
        if (args.colspan !== undefined) el.colSpan = args.colspan;
        if (args.rowspan !== undefined) el.rowSpan = args.rowspan;
        el.append(await render(args.body));
        return el;
      }
      if (name === 'table') {
        const columns = args.columns;
        if (!Number.isInteger(columns) || columns < 1 || columns > 256) throw Error('Invalid table columns');
        if (args.body?.item !== 'seq' || !Array.isArray(args.body.args?.children)) throw Error('Table body requires a sequence of table cells');
        if (args.body.args.children.length === 0) throw Error('Table requires at least one table-cell');
        const alignments = args.align == null ? Array(columns).fill('default') :
          typeof args.align === 'string' ? args.align.split(',').map(value => value.trim()) : [];
        if (alignments.length !== columns || !alignments.every(value => ['', 'default', 'left', 'center', 'right'].includes(value))) throw Error('Invalid table alignment');
        const table = annotate(document.createElement('table'));
        const occupied = Array(columns).fill(0);
        let cursor = 0, row = document.createElement('tr'), rowIndex = 0;
        const flush = () => { table.append(row); row = document.createElement('tr'); rowIndex++; occupied.forEach((n, i) => { occupied[i] = n - 1; }); cursor = 0; };
        for (const cell of args.body.args.children) {
          if (cell?.item !== 'table-cell') throw Error('Table body requires table-cell Items');
          const colspan = cell.args?.colspan ?? 1, rowspan = cell.args?.rowspan ?? 1;
          if (![colspan, rowspan].every(n => Number.isInteger(n) && n >= 1 && n <= 256)) throw Error('Invalid table-cell span');
          while (occupied.every(n => n > 0)) flush();
          const start = occupied.findIndex((n, i) => i >= cursor && n === 0);
          if (start < 0 || start + colspan > columns || occupied.slice(start, start + colspan).some(n => n > 0)) throw Error('Table cells overlap or exceed columns');
          occupied.fill(rowspan, start, start + colspan); cursor = start + colspan;
          const el = annotate(document.createElement(args.header === true && rowIndex === 0 ? 'th' : 'td'), cell);
          if (el.tagName === 'TH') el.scope = 'col';
          if (['left', 'center', 'right'].includes(alignments[start])) el.style.textAlign = alignments[start];
          el.colSpan = colspan; el.rowSpan = rowspan;
          if (!cell.args?.body) throw Error('Missing table-cell body');
          el.append(await render(cell.args.body)); row.append(el);
        }
        while (occupied.every(n => n > 0)) flush();
        if (occupied.some(n => n > 0)) throw Error('Table ends with an incomplete row');
        return table;
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
