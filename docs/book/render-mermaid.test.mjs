import test from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { closeOwnedBrowser, validatePng } from './render-mermaid.mjs';

test('graceful browser cleanup never kills a process', async () => {
  const child = { kill() { assert.fail('unexpected kill'); } };
  assert.equal(await closeOwnedBrowser({ process: () => child, close: async () => {} }, 10), 'graceful');
});

test('hung cleanup kills and reaps only the owned child', async () => {
  const child = new EventEmitter();
  child.exitCode = null;
  child.signalCode = null;
  child.kill = signal => {
    assert.equal(signal, 'SIGKILL');
    child.signalCode = signal;
    child.emit('exit');
  };
  assert.equal(await closeOwnedBrowser({ process: () => child,
    close: () => new Promise(() => {}) }, 10), 'forced');
});

test('foreign browser and unacknowledged termination fail closed', async () => {
  await assert.rejects(closeOwnedBrowser({ process: () => null }), /own/);
  const child = Object.assign(new EventEmitter(), { exitCode: null, signalCode: null, kill() {} });
  await assert.rejects(closeOwnedBrowser({ process: () => child,
    close: async () => { throw new Error('closed connection'); } }, 10), /did not exit/);
});

test('missing, truncated and zero-size PNGs are rejected', () => {
  const bytes = Buffer.alloc(45);
  Buffer.from('89504e470d0a1a0a', 'hex').copy(bytes);
  bytes.write('IHDR', 12);
  bytes.writeUInt32BE(100, 16);
  bytes.writeUInt32BE(50, 20);
  bytes.write('IEND', 37);
  validatePng(bytes);
  for (const invalid of [Buffer.alloc(0), bytes.subarray(0, 30), Buffer.alloc(45)]) {
    assert.throws(() => validatePng(invalid));
  }
  bytes.writeUInt32BE(0, 20);
  assert.throws(() => validatePng(bytes));
});
