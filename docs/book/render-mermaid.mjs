// Own one renderer browser per diagram and bound its post-render cleanup.
import { execFileSync } from 'node:child_process';
import { mkdtemp, readFile, realpath, rename, rm } from 'node:fs/promises';
import { createRequire } from 'node:module';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

async function within(promise, milliseconds) {
  let timer;
  try {
    return await Promise.race([promise, new Promise(resolve => {
      timer = setTimeout(() => resolve(false), milliseconds);
    })]);
  } finally {
    clearTimeout(timer);
  }
}

export async function closeOwnedBrowser(browser, graceMs = 5000) {
  const child = browser.process();
  if (!child) throw new Error('Renderer must own its browser process');
  const closed = browser.close().then(() => true, () => false);
  if (await within(closed, graceMs)) return 'graceful';
  // Kill only the ChildProcess returned by our launch, never a global browser.
  if (child.exitCode === null && child.signalCode === null) {
    const exited = new Promise(resolve => child.once('exit', () => resolve(true)));
    child.kill('SIGKILL');
    if (!await within(exited, graceMs)) throw new Error('Renderer browser did not exit');
  }
  return 'forced';
}

export function validatePng(bytes) {
  if (bytes.length < 45 || !bytes.subarray(0, 8).equals(Buffer.from('89504e470d0a1a0a', 'hex'))
      || bytes.toString('ascii', 12, 16) !== 'IHDR'
      || bytes.readUInt32BE(16) === 0 || bytes.readUInt32BE(20) === 0
      || bytes.toString('ascii', bytes.length - 8, bytes.length - 4) !== 'IEND') {
    throw new Error('Renderer did not produce a structurally valid PNG');
  }
}

async function main(input, output, config) {
  if (!input || !output || !config) throw new Error('Expected input, output and Puppeteer config');
  const cli = await realpath(execFileSync('which', ['mmdc'], { encoding: 'utf8' }).trim());
  const entry = path.join(path.dirname(cli), 'index.js');
  const { run } = await import(pathToFileURL(entry));
  const require = createRequire(entry);
  const puppeteer = require('puppeteer');
  const settings = JSON.parse(await readFile(config, 'utf8'));
  const directory = await mkdtemp(path.join(path.dirname(output), '.mermaid-'));
  const candidate = path.join(directory, 'diagram.png');
  let browser;
  try {
    browser = await puppeteer.launch({ headless: 'shell', ...settings });
    await run(input, candidate, { browser, parseMMDOptions: {
      viewport: { width: 800, height: 600, deviceScaleFactor: 2 }, backgroundColor: 'transparent',
    } });
    validatePng(await readFile(candidate));
    console.log(JSON.stringify({ event: 'diagram-rendered', input }));
    const cleanup = await closeOwnedBrowser(browser);
    browser = null;
    await rename(candidate, output);
    console.log(JSON.stringify({ event: 'diagram-complete', output, cleanup }));
  } finally {
    try {
      if (browser) await closeOwnedBrowser(browser);
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  await main(...process.argv.slice(2));
}
