// Onboarding — a fullscreen first-run wizard. The terminal is cleared and
// this module owns the whole screen until done: starfield warp → bloom →
// galaxy dissolve → WELCOME TO COSMOS → name → BYOK provider → key paste or
// ChatGPT browser sign-in → live model pick (fetched from the provider's
// /models) → typed tour. Runs once per ONBOARDING_VERSION; the module never
// loads again once the marker matches.
import React, { useEffect, useMemo, useRef, useState } from "react";
import { Box, Text, useInput, useStdout } from "ink";
import { DIM, GRAY, ROSE, SILVER } from "../theme";
import { Galaxy } from "../components/Galaxy";
import { BYOK, type ByokProvider } from "../byok";
import { fetchModels } from "../models";
import type { ZdevDriver } from "./driver";

type Phase =
  | "warp"
  | "reveal"
  | "name"
  | "byok"
  | "key"
  | "oauth"
  | "models"
  | "howto";

const clearScreen = () => {
  try {
    process.stdout.write("\x1b[2J\x1b[H");
  } catch {}
};

// ── starfield warp — fills the whole terminal ───────────────────────────────
type Star = { a: number; r: number; v: number };
const G_BY_R = ["·", "✦", "░", "▒", "▓"];
const WARP_FRAMES = 22;

function warpFrame(stars: Star[], w: number, h: number): string[] {
  const grid: string[][] = Array.from({ length: h }, () =>
    new Array<string>(w).fill(" "),
  );
  for (const s of stars) {
    s.r *= 1 + s.v * 0.22;
    const x = Math.round(w / 2 + Math.cos(s.a) * s.r * 2.1);
    const y = Math.round(h / 2 + Math.sin(s.a) * s.r * 0.55);
    if (x < 0 || x >= w || y < 0 || y >= h) {
      s.a = Math.random() * Math.PI * 2;
      s.r = 0.4 + Math.random() * 2;
      s.v = 0.3 + Math.random() * 1.1;
      continue;
    }
    const g = G_BY_R[Math.min(G_BY_R.length - 1, Math.floor(s.r / 5))];
    const cur = grid[y][x];
    if (cur === " " || G_BY_R.indexOf(g) > G_BY_R.indexOf(cur)) grid[y][x] = g;
  }
  return grid.map((row) => row.join(""));
}

function Warp({ onDone }: { onDone: () => void }) {
  const { stdout } = useStdout();
  const w = Math.max(30, (stdout?.columns ?? 80) - 4);
  const h = Math.max(10, (stdout?.rows ?? 24) - 4);
  const stars = useMemo<Star[]>(
    () =>
      Array.from({ length: Math.floor((w * h) / 26) }, () => ({
        a: Math.random() * Math.PI * 2,
        r: 0.4 + Math.random() * 6,
        v: 0.3 + Math.random() * 1.1,
      })),
    [w, h],
  );
  const [frame, setFrame] = useState(0);
  const done = useRef(false);
  const finish = () => {
    if (!done.current) {
      done.current = true;
      onDone();
    }
  };
  useInput(() => finish(), { isActive: !!process.stdin.isTTY });
  useEffect(() => {
    if (frame >= WARP_FRAMES) return finish();
    const id = setTimeout(() => setFrame((f) => f + 1), 70);
    return () => clearTimeout(id);
  }, [frame]);
  const bloom = frame >= WARP_FRAMES - 2;
  if (bloom && frame === WARP_FRAMES - 2) {
    try {
      process.stdout.write("\x07"); // soft terminal bell at the bloom
    } catch {}
  }
  const rows = bloom
    ? new Array<string>(h).fill("█".repeat(w))
    : warpFrame(stars, w, h);
  return (
    <Box flexDirection="column">
      {rows.map((r, i) => (
        <Text key={i} color={bloom ? "#ffffff" : "#9a9a9a"}>
          {r}
        </Text>
      ))}
      <Text color={DIM}>any key to skip</Text>
    </Box>
  );
}

// ── reveal: galaxy dissolves in row-by-row, then the title types ────────────
function Reveal({ width, onDone }: { width: number; onDone: () => void }) {
  const gw = Math.max(30, Math.min(52, width - 12));
  const [rows, setRows] = useState(0);
  const [title, setTitle] = useState(0);
  const TITLE = "W E L C O M E   T O   C O S M O S";
  useEffect(() => {
    if (rows < 13) {
      const id = setTimeout(() => setRows((r) => r + 1), 50);
      return () => clearTimeout(id);
    }
    if (title <= TITLE.length) {
      const id = setTimeout(() => setTitle((t) => t + 1), 28);
      return () => clearTimeout(id);
    }
    const id = setTimeout(onDone, 600);
    return () => clearTimeout(id);
  }, [rows, title, onDone]);
  return (
    <Box flexDirection="column" alignItems="center">
      <Box flexDirection="column" height={13}>
        {rows >= 13 ? (
          <Galaxy width={gw} height={13} />
        ) : (
          Array.from({ length: rows }).map((_, i) => (
            <Text key={i} color={DIM}>
              {"░▒▓·".repeat(24).slice(0, gw)}
            </Text>
          ))
        )}
      </Box>
      <Text> </Text>
      <Text bold color={ROSE}>
        {TITLE.slice(0, title)}
        {title < TITLE.length ? <Text color={SILVER}>▌</Text> : null}
      </Text>
      {title >= TITLE.length ? (
        <Text color={GRAY}>one session vault · every harness · your keys</Text>
      ) : null}
    </Box>
  );
}

// ── inputs ───────────────────────────────────────────────────────────────────
function LineInput({
  prompt,
  mask,
  initial = "",
  onSubmit,
  onEsc,
}: {
  prompt: string;
  mask?: boolean;
  initial?: string;
  onSubmit: (v: string) => void;
  onEsc?: () => void;
}) {
  const [v, setV] = useState(initial);
  useInput(
    (input, key) => {
      if (key.escape) return onEsc?.();
      if (key.return || /^[\r\n]+$/.test(input)) return onSubmit(v.trim());
      if (key.backspace || input === "\x7f") return setV((s) => s.slice(0, -1));
      if (input && !key.ctrl && !key.meta) {
        const clean = input.replace(/[\r\n]+/g, "");
        if (clean) setV((s) => s + clean);
      }
    },
    { isActive: !!process.stdin.isTTY },
  );
  return (
    <Box>
      <Text color={ROSE}>{prompt} </Text>
      <Text>
        {mask ? "•".repeat(v.length) : v}
        <Text color={SILVER}>▌</Text>
      </Text>
    </Box>
  );
}

/** Radio list — generic: renders {label, hint} rows with a scrolling window. */
function RadioList<T extends { label: string; hint?: string }>({
  list,
  sel,
  win = 12,
}: {
  list: T[];
  sel: number;
  win?: number;
}) {
  const w = Math.max(0, Math.min(sel - win + 1, list.length - win));
  const slice = list.slice(w, w + win);
  const maxLabel = Math.min(30, Math.max(...list.map((p) => p.label.length)));
  return (
    <Box flexDirection="column">
      {slice.map((p, i) => {
        const on = w + i === sel;
        return (
          <Text key={p.label} wrap="truncate-end">
            <Text color={on ? ROSE : DIM}>{on ? "❯ " : "  "}</Text>
            <Text bold={on} color={on ? undefined : GRAY}>
              {p.label.padEnd(maxLabel + 2)}
            </Text>
            <Text color={DIM}>{p.hint ?? ""}</Text>
          </Text>
        );
      })}
      {list.length > win ? (
        <Text color={DIM}>
          {"  "}↑ {w > 0 ? "…" : " "} · ↓ {w + win < list.length ? "…" : " "}
        </Text>
      ) : null}
    </Box>
  );
}

// ── the wizard ───────────────────────────────────────────────────────────────
export function Onboarding({
  driver,
  currentModel,
  onDone,
  onQuit,
}: {
  driver: ZdevDriver;
  currentModel: string;
  onDone: (pickedModel?: string) => void;
  onQuit: () => void;
}) {
  const [phase, setPhase] = useState<Phase>("warp");
  const [lines, setLines] = useState<string[]>([]);
  const [sel, setSel] = useState(0);
  const [provider, setProvider] = useState<ByokProvider | null>(null);
  const [oauthMsg, setOauthMsg] = useState("opening your browser…");
  const [models, setModels] = useState<string[]>([]);
  const [modelSel, setModelSel] = useState(0);
  const [modelsBusy, setModelsBusy] = useState(false);
  const [howtoIdx, setHowtoIdx] = useState(0);
  const { stdout } = useStdout();
  const doneRef = useRef(false);
  const nameHint = process.env.USERNAME ?? process.env.USER ?? "operator";
  const cols = stdout?.columns ?? 80;
  const rows = stdout?.rows ?? 24;

  const commit = (l: string) => setLines((prev) => [...prev, l]);
  const finish = (picked?: string) => {
    if (doneRef.current) return;
    doneRef.current = true;
    try {
      driver.markOnboarded();
    } catch {}
    clearScreen(); // hand a clean terminal back to the app
    onDone(picked);
  };

  // own the whole screen from mount to finish
  useEffect(() => {
    clearScreen();
  }, []);

  const HOWTO = useMemo(
    () => [
      "· type a task, hit enter — I work it end to end",
      "· /model picks a model · /plan is read-only · /yolo asks nothing",
      "· ctrl+o expands a tool call · @file embeds a file · ctrl+m dictates",
      "· esc interrupts · /resume reopens an old session · /help lists all",
      "· everything lands in ~/.cosmos-cli — zcli and zagy see it too",
    ],
    [],
  );

  useEffect(() => {
    if (phase !== "howto") return;
    if (howtoIdx >= HOWTO.length) {
      const id = setTimeout(() => finish(provider?.alias), 800);
      return () => clearTimeout(id);
    }
    const id = setTimeout(() => {
      commit(HOWTO[howtoIdx]);
      setHowtoIdx((i) => i + 1);
    }, 200);
    return () => clearTimeout(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [phase, howtoIdx]);

  /** persist provider + chosen model + set default, then → howto. */
  const commitProvider = (p: ByokProvider, modelId: string) => {
    driver.persistProvider({
      name: p.id,
      kind: p.kind,
      base_url: p.base_url,
      api_key_env: p.env,
      alias: p.alias,
      model_id: modelId,
      context: p.context,
    });
    driver.persistDefaultModel(p.alias);
    commit(`· ${modelId} → default model ${p.alias}`);
    setPhase("howto");
  };

  /** fetch live model ids, then show the picker. */
  const toModels = (p: ByokProvider) => {
    setProvider(p);
    setPhase("models");
    setModelsBusy(true);
    setModels([]);
    setModelSel(0);
    void (async () => {
      try {
        const key = p.env ? (process.env[p.env] ?? "") : "";
        const list = await fetchModels(p, key);
        setModels(list);
        // preselect the catalog default if the provider returned it
        const i = list.indexOf(p.model);
        if (i >= 0) setModelSel(i);
      } catch {
        setModels([]); // picker shows manual entry
      } finally {
        setModelsBusy(false);
      }
    })();
  };

  const pickProvider = (p: ByokProvider) => {
    commit(`· ${p.label}`);
    if (p.oauth) {
      setProvider(p);
      setPhase("oauth");
      setOauthMsg("opening your browser — sign in with ChatGPT…");
      void (async () => {
        try {
          const r = await driver.oauthLogin((u) =>
            setOauthMsg(`browser opened — or paste:\n${u}`),
          );
          commit(`· signed in${r.email ? ` as ${r.email}` : ""}`);
          toModels(p);
        } catch (e: any) {
          commit(`· sign-in failed: ${e?.message ?? e} — pick another`);
          setPhase("byok");
        }
      })();
      return;
    }
    if (!p.env || process.env[p.env]) {
      // keyless (ollama/fixture) or key already in env — straight to models
      if (!p.env && p.id === "fixture") {
        driver.persistDefaultModel("fixture");
        commit("· offline fixture — no provider");
        setPhase("howto");
        return;
      }
      toModels(p);
      return;
    }
    setProvider(p);
    setPhase("key");
  };

  useInput(
    (input, key) => {
      if (key.ctrl && (input === "c" || input === "\x03")) return onQuit();
      if (phase === "warp" || phase === "reveal") return;
      if (key.escape) {
        if (phase === "byok") return finish(currentModel.split(" ")[0]);
        if (phase === "key" || phase === "models" || phase === "oauth")
          return setPhase("byok");
        return finish();
      }
      if (phase === "byok") {
        if (key.downArrow) setSel((s) => (s + 1) % BYOK.length);
        else if (key.upArrow)
          setSel((s) => (s - 1 + BYOK.length) % BYOK.length);
        else if (key.return || /^[\r\n]+$/.test(input)) pickProvider(BYOK[sel]);
        return;
      }
      if (phase === "models" && models.length) {
        if (key.downArrow) setModelSel((s) => (s + 1) % models.length);
        else if (key.upArrow)
          setModelSel((s) => (s - 1 + models.length) % models.length);
        else if (key.return || /^[\r\n]+$/.test(input)) {
          commitProvider(provider!, models[modelSel]);
        }
      }
    },
    { isActive: !!process.stdin.isTTY },
  );

  return (
    <Box
      flexDirection="column"
      width={cols - 1}
      minHeight={rows - 2}
      justifyContent="center"
      alignItems="center"
    >
      {phase === "warp" && <Warp onDone={() => setPhase("reveal")} />}
      {phase === "reveal" && (
        <Reveal width={cols} onDone={() => setPhase("name")} />
      )}

      {phase !== "warp" && phase !== "reveal" && (
        <Box flexDirection="column" width={Math.min(72, cols - 8)}>
          {lines.map((l, i) => (
            <Text key={i} color={l.startsWith("·") ? GRAY : undefined}>
              {l}
            </Text>
          ))}

          {phase === "name" && (
            <>
              <Text>hey — first run. what should I call you?</Text>
              <LineInput
                prompt="❯"
                initial={nameHint}
                onSubmit={(v) => {
                  const name = v || nameHint;
                  driver.persistName(name);
                  commit(`hey — first run. what should I call you?\n❯ ${name}`);
                  commit(`· welcome aboard, ${name}`);
                  commit("now — how do you want to power me?");
                  setPhase("byok");
                }}
                onEsc={() => {
                  commit("· keeping it anonymous");
                  commit("now — how do you want to power me?");
                  setPhase("byok");
                }}
              />
            </>
          )}

          {phase === "byok" && (
            <>
              <Text bold>pick a brain — subscription or any API key:</Text>
              <Text> </Text>
              <RadioList list={BYOK} sel={sel} win={Math.min(14, rows - 10)} />
              <Text> </Text>
              <Text color={DIM}>
                ↑/↓ move · enter select · esc keeps fixture
              </Text>
            </>
          )}

          {phase === "key" && provider && (
            <>
              <Text> </Text>
              <Text>
                paste your {provider.label} key — it goes to{" "}
                <Text bold>{provider.env}</Text> (user env, never config):
              </Text>
              <LineInput
                prompt="❯"
                mask
                onSubmit={(v) => {
                  if (!v) return setPhase("byok");
                  driver.persistEnvVar(provider.env, v);
                  commit(`· ${provider.env} saved`);
                  toModels(provider);
                }}
                onEsc={() => setPhase("byok")}
              />
            </>
          )}

          {phase === "oauth" && (
            <>
              <Text> </Text>
              <Text color={GRAY}>{oauthMsg}</Text>
            </>
          )}

          {phase === "models" && provider && (
            <>
              <Text> </Text>
              {modelsBusy ? (
                <Text color={GRAY}>fetching models from {provider.label}…</Text>
              ) : models.length ? (
                <>
                  <Text bold>
                    pick a model ({models.length} live from {provider.label}):
                  </Text>
                  <RadioList
                    list={models.map((m) => ({ label: m }))}
                    sel={modelSel}
                    win={Math.min(12, rows - 12)}
                  />
                  <Text color={DIM}>
                    ↑/↓ move · enter select · esc back
                  </Text>
                </>
              ) : (
                <>
                  <Text color={GRAY}>
                    couldn't fetch models — type an id, enter keeps{" "}
                    {provider.model}:
                  </Text>
                  <LineInput
                    prompt="❯"
                    initial=""
                    onSubmit={(v) =>
                      commitProvider(provider, v || provider.model)
                    }
                    onEsc={() => setPhase("byok")}
                  />
                </>
              )}
            </>
          )}
        </Box>
      )}
    </Box>
  );
}
