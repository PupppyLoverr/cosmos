// TUI entry — renders <App/> driving the Agent in-process via ZdevDriver.
import React from "react";
import { render } from "ink";
import { App } from "./app";
import type { RunOpts } from "../proto";
import { EXIT_DONE } from "../proto";

export async function runTui(opts: RunOpts): Promise<number> {
  const boot = {
    cwd: opts.cwd ?? process.cwd(),
    model: opts.model ?? undefined,
    mode: opts.mode ?? (opts.yolo ? "yolo" : undefined),
    opts,
  };
  const app = render(<App boot={boot} />, {
    exitOnCtrlC: false, // we handle ctrl+c ourselves (also ctrl+d)
  });
  await app.waitUntilExit();
  return EXIT_DONE;
}
