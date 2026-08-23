#!/usr/bin/env node

const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

const root = path.resolve(__dirname, '..');
const babelSource = fs.readFileSync(path.join(root, 'tools/vendor/babel.min.js'), 'utf8');
const context = { console, process, setTimeout, clearTimeout };
vm.createContext(context);
vm.runInContext(babelSource, context, { filename: 'babel.min.js' });
const { Babel } = context;
const presets = ['react', ['env', { targets: { esmodules: true }, bugfixes: true, modules: false }]];

function compile(input, output) {
  const source = fs.readFileSync(input, 'utf8');
  const result = Babel.transform(source, {
    filename: path.relative(root, input),
    presets,
    sourceMaps: false,
    comments: true,
  });
  fs.writeFileSync(output, `(function () {\n${result.code}\n})();\n`);
}

function rewriteScripts(input, output) {
  let html = fs.readFileSync(input, 'utf8');
  html = html.replace(/<script type="text\/babel" src="([^"]+?)"><\/script>/g, (_, src) => {
    const compiled = src.replace(/\.jsx$/, '.js');
    return `<script src="${compiled}"></script>`;
  });
  html = html.replace(/<script type="text\/babel">([\s\S]*?)<\/script>/g, (_, source) => {
    const result = Babel.transform(source, {
      filename: path.relative(root, input),
      presets,
      sourceMaps: false,
      comments: true,
    });
    return `<script>\n(function () {\n${result.code}\n})();\n</script>`;
  });
  fs.writeFileSync(output, html);
}

function main() {
  const dashboard = path.join(root, 'webui/ui_kits/dashboard');
  for (const file of fs.readdirSync(dashboard).filter((name) => name.endsWith('.jsx') && !name.startsWith('._'))) {
    compile(path.join(dashboard, file), path.join(dashboard, file.replace(/\.jsx$/, '.js')));
  }
  compile(path.join(root, 'webui/hosted/Admin.jsx'), path.join(root, 'webui/hosted/Admin.js'));
  rewriteScripts(path.join(root, 'tools/templates/dashboard-index.html'), path.join(dashboard, 'index.html'));
  rewriteScripts(path.join(root, 'tools/templates/hosted-index.html'), path.join(root, 'webui/hosted/index.html'));
}

main();
