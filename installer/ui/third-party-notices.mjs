import { readFile, readdir } from 'node:fs/promises';

// Include notices for modules actually retained in the production bundle.
export function thirdPartyNotices() {
  return {
    name: 'installer-third-party-notices',
    async generateBundle(_, bundle) {
      const roots = new Set();
      for (const output of Object.values(bundle)) {
        if (output.type !== 'chunk') continue;
        for (const [id, module] of Object.entries(output.modules)) {
          if (!module.renderedLength) continue;
          const normalized = id.replaceAll('\\', '/');
          const marker = normalized.lastIndexOf('/node_modules/');
          if (marker < 0) continue;
          const start = marker + '/node_modules/'.length;
          const parts = normalized.slice(start).split('/');
          const name = parts[0].startsWith('@') ? parts.slice(0, 2).join('/') : parts[0];
          roots.add(normalized.slice(0, start) + name);
        }
      }
      const grouped = new Map();
      for (const root of [...roots].sort()) {
        const pkg = JSON.parse(await readFile(root + '/package.json', 'utf8'));
        const names = await readdir(root);
        const licenseName = names.find(name => /^licen[cs]e(?:\.md|\.txt)?$/i.test(name));
        const licensePath = licenseName ? root + '/' + licenseName
          : pkg.name === '@fluentui/react-icons' ? new URL('./licenses/fluentui-react-icons.txt', import.meta.url) : null;
        if (!licensePath) throw new Error('Missing third-party license: ' + pkg.name);
        const text = (await readFile(licensePath, 'utf8')).replaceAll('\r\n', '\n').trim();
        const packages = grouped.get(text) ?? [];
        packages.push(pkg.name + '@' + pkg.version);
        grouped.set(text, packages);
      }
      const source = 'Glide-Dock installer — third-party software notices\n\n'
        + [...grouped].map(([license, packages]) => packages.join('\n') + '\n\n' + license).join('\n\n' + '='.repeat(72) + '\n\n') + '\n';
      this.emitFile({ type: 'asset', fileName: 'THIRD-PARTY-NOTICES.txt', source });
    },
  };
}
