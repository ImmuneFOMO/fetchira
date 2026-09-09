#!/usr/bin/env node

const fs = require('node:fs');
const path = require('node:path');

const root = path.resolve(__dirname, '..');
const htmlFiles = [
  path.join(root, 'webui/hosted/index.html'),
  path.join(root, 'webui/ui_kits/dashboard/index.html'),
];

const errors = [];
for (const htmlFile of htmlFiles) {
  const html = fs.readFileSync(htmlFile, 'utf8');
  if (/text\/babel|vendor\/babel|\.jsx"/.test(html)) {
    errors.push(`${path.relative(root, htmlFile)} still references runtime Babel or JSX`);
  }
  const base = path.dirname(htmlFile);
  const references = [...html.matchAll(/(?:src|href)="([^"]+)"/g)]
    .map((match) => match[1])
    .filter((asset) => !asset.startsWith('http://') && !asset.startsWith('https://') && !asset.startsWith('#'));
  for (const asset of references) {
    const relative = asset.startsWith('/')
      ? path.join(root, 'webui', asset.replace(/^\/admin\/assets\//, ''))
      : path.resolve(base, asset);
    if (!fs.existsSync(relative)) errors.push(`${path.relative(root, htmlFile)} -> ${asset} (missing ${path.relative(root, relative)})`);
  }
}

if (errors.length) {
  console.error(errors.join('\n'));
  process.exit(1);
}
console.log(`webui asset smoke passed: ${htmlFiles.length} entrypoints`);
