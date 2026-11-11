import fs from 'node:fs';
import path from 'node:path';

const noticeName = /^(?:LICENSE|LICENCE|NOTICE|COPYING)(?:[._-].*)?$/i;

function packageRoot(moduleId) {
  const clean = moduleId.split('?', 1)[0].replaceAll('\\', '/');
  const marker = '/node_modules/';
  const index = clean.lastIndexOf(marker);
  if (index < 0) return null;
  const start = index + marker.length;
  const parts = clean.slice(start).split('/');
  if (!parts[0] || parts[0].startsWith('.')) return null;
  const count = parts[0].startsWith('@') ? 2 : 1;
  if (parts.length <= count || !parts[count - 1]) return null;
  return clean.slice(0, start) + parts.slice(0, count).join('/');
}

export function collectWebNotices(moduleIds) {
  const roots = new Set([...moduleIds].map(packageRoot).filter(Boolean));
  const entries = [];
  for (const root of [...roots].sort()) {
    const manifest = JSON.parse(fs.readFileSync(path.join(root, 'package.json'), 'utf8'));
    const files = fs.readdirSync(root, { withFileTypes: true })
      .filter((item) => item.isFile() && noticeName.test(item.name))
      .map((item) => item.name).sort();
    if (files.length === 0) {
      throw new Error(`No root license or notice file for bundled package ${manifest.name}@${manifest.version}`);
    }
    entries.push({ name: manifest.name, version: manifest.version, root, files });
  }
  entries.sort((a, b) => `${a.name}@${a.version}`.localeCompare(`${b.name}@${b.version}`));
  let output = 'Third-party notices for bundled web JavaScript\n';
  for (const entry of entries) {
    for (const file of entry.files) {
      output += `\n=== ${entry.name}@${entry.version} / ${file} ===\n`;
      const notice = fs.readFileSync(path.join(entry.root, file), 'utf8');
      output += notice;
      if (!notice.endsWith('\n')) output += '\n';
    }
  }
  return output;
}

export function webNoticesPlugin() {
  return {
    name: 'web-third-party-notices',
    generateBundle(_options, bundle) {
      const modules = Object.values(bundle)
        .filter((item) => item.type === 'chunk')
        .flatMap((chunk) => Object.keys(chunk.modules));
      this.emitFile({
        type: 'asset',
        fileName: 'licenses/third-party.txt',
        source: collectWebNotices(modules),
      });
    },
  };
}
