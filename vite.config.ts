import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import path from 'path';

export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      '@': path.resolve(__dirname, './src'),
    },
  },
  server: {
    port: 5173,
    strictPort: false,
    // HMR disabled for now: this project lives inside a OneDrive-synced
    // folder, and the Rust build artifacts in cli/target/ are marked as
    // cloud placeholder files. Chokidar's initial directory walk crashes
    // when it tries to lstat them ("cloud file provider is not running").
    //
    // Workaround: disable the file watcher entirely. You'll need to do a
    // manual browser refresh after edits. If you want HMR back, either:
    //   (a) move the project out of OneDrive, OR
    //   (b) set CARGO_TARGET_DIR to a non-OneDrive path, AND delete
    //       cli/target/ with OneDrive running, OR
    //   (c) pause OneDrive sync for the BugJuice folder.
    watch: null,
  },
});
