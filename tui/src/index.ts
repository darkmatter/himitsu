import { createCliRenderer } from "@opentui/core";
import { createApp } from "./app";
import type { WizardDefaults } from "./views/init-wizard";

// Parse CLI arguments
let remote: string | undefined;
let initMode = false;
let initDefaults: WizardDefaults | undefined;

const args = process.argv.slice(2);
for (let i = 0; i < args.length; i++) {
  if (args[i] === "--init") {
    initMode = true;
    // Next arg is the defaults JSON (if present)
    if (args[i + 1] && !args[i + 1].startsWith("--")) {
      try {
        initDefaults = JSON.parse(args[i + 1]) as WizardDefaults;
      } catch {
        // ignore parse errors, wizard will use fallbacks
      }
      i++;
    }
  } else if (!args[i].startsWith("--")) {
    remote = args[i];
  }
}

(async () => {
  const renderer = await createCliRenderer({
    exitOnCtrlC: true,
    targetFps: 30,
  });

  const app = createApp(renderer, remote, { initMode, initDefaults });
  renderer.root.add(app);
})();
