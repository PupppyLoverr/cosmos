// TUI entry — first-run onboarding (full screen, before Ink starts), then
// <App/> driving the Agent in-process via ZdevDriver. `/onboarding` exits
// the app, replays the flow and starts it again.
import React from "react";
import { render } from "ink";

import type { RunOpts } from "../proto";
import { EXIT_DONE } from "../proto";
import { App, type ExitReason } from "./app";
import { ZdevDriver } from "./driver";

/** The welcome flow; false = the operator pressed Ctrl+C. */
async function welcome(): Promise<boolean> {
  const { runWelcome } = await import("../welcome/index");
  const ok = await runWelcome();
  if (ok) ZdevDriver.markOnboarded();
  return ok;
}

export async function runTui(opts: RunOpts): Promise<number> {
  const needed = !ZdevDriver.onboardingDone() && !process.env.ZDEV_NO_ONBOARDING;
  if (needed && !(await welcome())) return EXIT_DONE;
  for (;;) {
    const boot = {
      cwd: opts.cwd ?? process.cwd(),
      // the welcome flow may have just written a default model
      model: opts.model ?? undefined,
      mode: opts.mode ?? (opts.yolo ? "yolo" : undefined),
      opts,
    };
    let reason = "quit" as ExitReason;
    const app = render(<App boot={boot} onExit={(r) => (reason = r)} />, {
      exitOnCtrlC: false, // ctrl+c clears, interrupts, then quits on a second press
      patchConsole: true, // stray console output can't tear the frame
    });
    await app.waitUntilExit();
    if (reason !== "onboarding") return EXIT_DONE;
    if (!(await welcome())) return EXIT_DONE;
  }
}

/** `zdev onboarding` — replay the first-run flow standalone. */
export async function runOnboarding(): Promise<number> {
  await welcome();
  return EXIT_DONE;
}
