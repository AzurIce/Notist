// Shadertoy-like Web Component for Notist shader plugin.
// The HTML renderer emits <notist-shader data-shader-source="...">; this
// module upgrades it into a real custom element with Shadow DOM.

const VERT = `@vertex fn vs_main(@builtin(vertex_index) vid: u32) -> @builtin(position) vec4<f32> {
  var pos = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
  return vec4<f32>(pos[vid], 0.0, 1.0);
}`;

const FRAG_TAIL = `
@fragment fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
  return mainImage(pos.xy);
}`;

class NotistShader extends HTMLElement {
  connectedCallback() {
    const source = this.dataset.shaderSource || '';
    const width = parseInt(this.dataset.width || '800', 10);
    const height = parseInt(this.dataset.height || '600', 10);

    const shadow = this.attachShadow({ mode: 'open' });
    shadow.innerHTML = `
      <style>
        :host {
          display: block;
          margin: 1rem 0;
        }
        canvas {
          width: 100%;
          height: auto;
          border: 1px solid rgb(0 0 0 / 0.15);
          border-radius: 8px;
          background: #000;
        }
        slot {
          display: block;
          margin-top: 0.5rem;
          color: rgb(0 0 0 / 0.65);
        }
      </style>
      <canvas width="${width}" height="${height}"></canvas>
      <slot></slot>
    `;

    if (!navigator.gpu) {
      const error = document.createElement('p');
      error.textContent = 'WebGPU is not available in this browser.';
      shadow.appendChild(error);
      return;
    }

    this._run(shadow.querySelector('canvas'), source);
  }

  async _run(canvas, source) {
    const adapter = await navigator.gpu.requestAdapter();
    const device = await adapter.requestDevice();
    const context = canvas.getContext('webgpu');
    const format = navigator.gpu.getPreferredCanvasFormat();
    context.configure({ device, format, alphaMode: 'opaque' });

    const module = device.createShaderModule({
      code: source + '\n' + VERT + '\n' + FRAG_TAIL,
    });

    const pipeline = device.createRenderPipeline({
      layout: 'auto',
      vertex: { module, entryPoint: 'vs_main' },
      fragment: { module, entryPoint: 'fs_main', targets: [{ format }] },
      primitive: { topology: 'triangle-list' },
    });

    const frame = () => {
      const encoder = device.createCommandEncoder();
      const pass = encoder.beginRenderPass({
        colorAttachments: [{
          view: context.getCurrentTexture().createView(),
          clearValue: { r: 0, g: 0, b: 0, a: 1 },
          loadOp: 'clear',
          storeOp: 'store',
        }],
      });
      pass.setPipeline(pipeline);
      pass.draw(3);
      pass.end();
      device.queue.submit([encoder.finish()]);
      requestAnimationFrame(frame);
    };

    frame();
  }
}

customElements.define('notist-shader', NotistShader);
