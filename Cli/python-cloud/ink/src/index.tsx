// zdev-ui — Ink TUI entry. Spawns `zdev acp`, renders <App/>.
//   zdev-ui [--model alias] [--mode plan|agent|yolo] [--cwd dir]
// Env: ZDEV_ACP_CMD overrides the backend launcher (default:
// `uv run -q --project <python-cloud> zdev acp`).
import React from "react";
import { render } from "ink";
import { App } from "./app";

function parseArgs(argv: string[]) {
  const boot: { cwd: string; model?: string; mode?: string } = {
    cwd: process.cwd(),
  };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    const v = argv[i + 1];
    if (a === "--model" && v) (boot.model = v), i++;
    else if (a === "--mode" && v) (boot.mode = v), i++;
    else if (a === "--cwd" && v) (boot.cwd = v), i++;
  }
  return boot;
}

const app = render(<App boot={parseArgs(process.argv.slice(2))} />, {
  exitOnCtrlC: false, // we handle ctrl+c ourselves (also ctrl+d)
});

process.on("SIGINT", () => {
  app.unmount();
  process.exit(0);
});
