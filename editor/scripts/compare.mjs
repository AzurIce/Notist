import { createRequire } from 'node:module';
import { execFileSync } from 'node:child_process';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import assert from 'node:assert/strict';

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, '../..');
const { analyze } = createRequire(import.meta.url)('./pkg-node/notist_editor.js');
const run = flag => JSON.parse(execFileSync('cargo', ['run', '-q', '-j4', '-p', 'notist-cli', '--', 'eval', 'examples/demo', flag], { cwd: root, encoding: 'utf8' }));
const request = run('--request');
const native = run('--snapshot');
const wasm = JSON.parse(analyze(JSON.stringify(request)));
native.platform = null;
wasm.platform = null;
// Source ids and timing are host details; the language result and diagnostics are the contract.
assert.deepEqual(wasm.result.content, native.result.content);
assert.deepEqual(wasm.evaluation.content, native.evaluation.content);
assert.deepEqual(wasm.result.diagnostics, native.result.diagnostics);
console.log('Native/WASM package results match.');
