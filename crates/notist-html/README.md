# Notist HTML

HTML consumer of `notist-ir::Content`. `render` and `RenderHtml` produce HTML strings; `RENDERER_JS` mounts serialized Content in the browser and loads package components. `BUNDLE_HTML` is the browser bundle entry page. CLI hosts own package loading, file output and HTTP serving.

Core Items render as native HTML. Other Items use custom elements implementing `render(args, context)`, with `context.renderContent` for nested Content. The package loader supplies the component map and resources; rendering failures remain local to the HTML consumer.
