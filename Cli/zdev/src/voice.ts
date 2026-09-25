// Mic dictation — Windows desktop speech recognition (System.Speech) via a
// short-lived PowerShell child. One utterance per press: ends on silence
// or the timeout. No native deps; degrades cleanly when no recognizer/mic.
import { spawn } from "node:child_process";

const SCRIPT = `
Add-Type -AssemblyName System.Speech
try {
  $r = New-Object System.Speech.Recognition.SpeechRecognitionEngine
  $r.SetInputToDefaultAudioDevice()
  $g = New-Object System.Speech.Recognition.DictationGrammar
  $r.LoadGrammar($g)
  $r.EndSilenceTimeout = [TimeSpan]::FromMilliseconds(900)
  $res = $r.Recognize([TimeSpan]::FromSeconds(14))
  if ($res -and $res.Text) { [Console]::Out.Write($res.Text) }
  $r.Dispose()
} catch {
  [Console]::Error.Write($_.Exception.Message)
  exit 1
}
`;

export type Dictation = { text?: string; error?: string };

/** Listen for one utterance; resolves with recognized text (may be empty). */
export function dictate(timeoutMs = 18_000): Promise<Dictation> {
  return new Promise((resolve) => {
    let out = "";
    let err = "";
    let done = false;
    const finish = (r: Dictation) => {
      if (!done) {
        done = true;
        resolve(r);
      }
    };
    let ps;
    try {
      ps = spawn(
        "powershell",
        ["-NoProfile", "-NonInteractive", "-Command", SCRIPT],
        { windowsHide: true },
      );
    } catch (e: any) {
      return finish({ error: `speech unavailable: ${e?.message ?? e}` });
    }
    const kill = setTimeout(() => {
      try {
        ps.kill();
      } catch {}
      finish({ error: "listening timed out" });
    }, timeoutMs);
    ps.stdout.on("data", (d) => (out += d.toString()));
    ps.stderr.on("data", (d) => (err += d.toString()));
    ps.on("error", (e) => {
      clearTimeout(kill);
      finish({ error: `speech unavailable: ${e.message}` });
    });
    ps.on("close", (code) => {
      clearTimeout(kill);
      if (out.trim()) finish({ text: out.trim() });
      else if (code !== 0)
        finish({
          error:
            err.trim() ||
            "no speech recognizer — install a Windows speech language pack",
        });
      else finish({ text: "" }); // silence — nothing heard
    });
  });
}
