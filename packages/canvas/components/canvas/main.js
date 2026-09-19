export default class Canvas extends HTMLElement {
  render(args) {
    const width = args.width ?? 640, height = args.height ?? 360;
    if (!Number.isInteger(width) || !Number.isInteger(height) || width < 1 || height < 1 || width > 4096 || height > 4096) throw Error('canvas dimensions must be integers from 1 to 4096');
    if (!Array.isArray(args.commands)) throw Error('canvas.commands must be List');
    const canvas = document.createElement('canvas');
    canvas.width = width; canvas.height = height;
    canvas.style.aspectRatio = `${width}/${height}`;
    canvas.setAttribute('aria-label', 'Canvas drawing');
    const ctx = canvas.getContext('2d');
    for (const value of args.commands) {
      const command = value.dict ?? value;
      ctx.fillStyle = command.color ?? '#247a70';
      const number = (name, fallback = 0) => {
        const n = command[name] ?? fallback;
        if (typeof n !== 'number' || !Number.isFinite(n)) throw Error(`canvas.${name} must be numeric`);
        return n;
      };
      switch (command.op) {
        case 'rect': ctx.fillRect(number('x'), number('y'), number('width'), number('height')); break;
        case 'circle': ctx.beginPath(); ctx.arc(number('x'), number('y'), number('radius'), 0, 2 * Math.PI); ctx.fill(); break;
        case 'text': ctx.font = `${number('size', 20)}px system-ui`; ctx.fillText(String(command.text ?? ''), number('x'), number('y')); break;
        default: throw Error(`unknown canvas operation: ${command.op}`);
      }
    }
    this.replaceChildren(canvas);
  }
}
