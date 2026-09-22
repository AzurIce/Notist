import init, { analyze } from "/language/notist_editor.js";
const ready = init();
self.onmessage = async ({ data: { revision, source } }) => {
  try {
    await ready;
    const snapshot = JSON.parse(analyze(JSON.stringify({
      request_id: String(revision), entry: "draft.not",
      files: { "draft.not": source }, binaries: {}, dependencies: {}, options: {},
    })));
    self.postMessage({ revision, snapshot });
  } catch (error) { self.postMessage({ revision, error: String(error) }); }
};
