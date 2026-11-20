// Pages and browser shared by the local benchmarks.

import { readdirSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';

export const PAGES = [
  { name: 'hn', url: 'https://news.ycombinator.com' },
  { name: 'wiki', url: 'https://en.wikipedia.org/wiki/Rust_(programming_language)' },
  { name: 'gh', url: 'https://github.com/rust-lang/rust' },
  { name: 'mdn', url: 'https://developer.mozilla.org/en-US/docs/Web/HTML/Element/input' },
  { name: 'form', url: 'https://httpbin.org/forms/post' },
  { name: 'bbc', url: 'https://www.bbc.com/news' },
  { name: 'amazon', url: 'https://www.amazon.com/s?k=usb+c+cable' },
  { name: 'gov', url: 'https://www.gov.uk/browse/driving' },
];

/** Chrome for Testing as installed by `frob install`, so every tool drives the same browser. */
export function chromePath() {
  if (process.env.BENCH_CHROME) return process.env.BENCH_CHROME;
  const dir = join(homedir(), '.frob/browsers');
  const version = readdirSync(dir).filter((d) => d.startsWith('chrome-') && !d.includes('headless')).sort().pop();
  if (!version) throw new Error('No Chrome for Testing under ~/.frob/browsers; run `frob install` or set BENCH_CHROME');
  return join(dir, version, 'Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing');
}
