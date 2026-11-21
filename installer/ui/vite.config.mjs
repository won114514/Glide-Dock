import { defineConfig } from 'vite';
import { thirdPartyNotices } from './third-party-notices.mjs';

export default defineConfig({
  base: './',
  plugins: [thirdPartyNotices()],
  build: {
    outDir: '../../target/installer-ui',
    emptyOutDir: true,
    assetsInlineLimit: 0,
    rolldownOptions: {
      onLog(level, log, handler) {
        // The installer runs entirely on the client; React Server Component directives do not apply.
        if (log.code === 'MODULE_LEVEL_DIRECTIVE' && log.message.includes('use client')) return;
        handler(level, log);
      },
      output: {
        codeSplitting: {
          groups: [
            { name: 'react', test: /node_modules[\\/](?:react(?:-dom)?|scheduler)[\\/]/ },
            { name: 'fluent', test: /node_modules[\\/](?:@fluentui|@griffel)[\\/]/ },
          ],
        },
      },
    },
  },
});
