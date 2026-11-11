import fs from 'node:fs';
import path from 'node:path';
import { randomBytes } from 'node:crypto';

function privateDirectory(directory) {
  const stat = fs.lstatSync(directory);
  if (!stat.isDirectory() || stat.isSymbolicLink()) throw new Error('symlink browser output refused');
  return stat;
}
function belowEvidence(directory, evidence) {
  const absolute = path.resolve(directory);
  const base = path.resolve(evidence);
  if (!absolute.startsWith(base + path.sep)) throw new Error('fresh owned browser output required');
  let current = path.parse(absolute).root;
  for (const part of absolute.split(path.sep).filter(Boolean)) {
    current = path.join(current, part);
    try { privateDirectory(current); }
    catch (error) { if (error.code !== 'ENOENT' || current !== absolute) throw error; }
  }
  return absolute;
}
export function createOutput(directory, evidence) {
  const absolute = belowEvidence(directory, evidence);
  // No reuse, including a dangling symlink. Parent must already exist and be owned
  // by the harness; never recursively make an arbitrary supplied pathname.
  fs.mkdirSync(absolute, { mode: 0o700 });
  const token = randomBytes(32).toString('hex');
  fs.writeFileSync(path.join(absolute, '.browser-owner'), token, { flag: 'wx', mode: 0o600 });
  const stat = privateDirectory(absolute);
  return JSON.stringify({ dev: stat.dev, ino: stat.ino, token });
}
export function verifyOutput(directory, ownership, evidence) {
  const absolute = belowEvidence(directory, evidence);
  if (!ownership) throw new Error('fresh owned browser output required');
  const owner = JSON.parse(ownership);
  const stat = privateDirectory(absolute);
  const marker = path.join(absolute, '.browser-owner');
  const m = fs.lstatSync(marker);
  if (stat.dev !== owner.dev || stat.ino !== owner.ino || (stat.mode & 0o077) !== 0 ||
      !m.isFile() || m.isSymbolicLink() || m.nlink !== 1 || (m.mode & 0o077) !== 0 ||
      fs.readFileSync(marker, 'utf8') !== owner.token) throw new Error('fresh owned browser output required');
  const artifacts = path.join(absolute, 'artifacts');
  // The launcher proved this tree was new; workers may observe the runner-created
  // artifacts directory, but no symlink can redirect Playwright's recursive cleanup.
  try { privateDirectory(artifacts); }
  catch (error) { if (error.code !== 'ENOENT') throw error; }
  return artifacts;
}
