// Grayscale syntax highlighting for code fences — zdev is black and white,
// so structure comes from weight and shade: keywords bright + bold, strings
// light, numbers mid, comments dim italic. A small tokenizer covers the
// C-family, Python, Rust, Go, shell, JSON/TOML/YAML well enough for chat.

export type Tok = { text: string; kind: "kw" | "str" | "num" | "com" | "fn" | "type" | "plain" | "punct" };

const KW = new Set(
  (
    "fn let mut const var function return if else for while loop match in of do break continue " +
    "class struct enum impl trait type interface extends implements pub use mod crate self super " +
    "import from export default as async await yield new delete typeof instanceof try catch finally " +
    "throw throws def lambda pass raise with global nonlocal and or not is None True False null " +
    "undefined true false nil package func go defer select case switch chan map range static final " +
    "void int bool string float double char unsafe where dyn ref move echo then fi elif esac done local"
  ).split(" "),
);

const LINE_COMMENT: Record<string, string> = {
  py: "#", python: "#", sh: "#", bash: "#", zsh: "#", shell: "#", toml: "#", yaml: "#", yml: "#",
  rb: "#", ruby: "#", ps1: "#", powershell: "#", r: "#", sql: "--", lua: "--", hs: "--",
};

export function tokenize(line: string, lang: string): Tok[] {
  const l = lang.toLowerCase();
  const lc = LINE_COMMENT[l] ?? "//";
  const out: Tok[] = [];
  let i = 0;
  const push = (text: string, kind: Tok["kind"]) => {
    if (!text) return;
    const last = out[out.length - 1];
    if (last && last.kind === kind) last.text += text;
    else out.push({ text, kind });
  };
  while (i < line.length) {
    const rest = line.slice(i);
    if (rest.startsWith(lc) || (lc === "//" && rest.startsWith("/*"))) {
      push(rest, "com");
      break;
    }
    const q = rest[0];
    if (q === '"' || q === "'" || q === "`") {
      let j = 1;
      while (j < rest.length && rest[j] !== q) j += rest[j] === "\\" ? 2 : 1;
      push(rest.slice(0, j + 1), "str");
      i += j + 1;
      continue;
    }
    const num = rest.match(/^(0x[0-9a-fA-F_]+|\d[\d_]*(\.\d+)?([eE][+-]?\d+)?)/);
    if (num && (i === 0 || !/[\w$]/.test(line[i - 1]))) {
      push(num[0], "num");
      i += num[0].length;
      continue;
    }
    const word = rest.match(/^[A-Za-z_$][\w$]*/);
    if (word) {
      const w = word[0];
      const next = rest.slice(w.length).trimStart()[0];
      push(w, KW.has(w) ? "kw" : next === "(" ? "fn" : /^[A-Z]/.test(w) ? "type" : "plain");
      i += w.length;
      continue;
    }
    push(rest[0], /[{}()[\];,.:<>=+\-*/%!&|^~?@#]/.test(rest[0]) ? "punct" : "plain");
    i += 1;
  }
  return out;
}

export const TOK_STYLE: Record<Tok["kind"], { color: string; bold?: boolean; italic?: boolean }> = {
  kw: { color: "#ffffff", bold: true },
  fn: { color: "#e6e6e6" },
  type: { color: "#d4d4d4", bold: true },
  str: { color: "#b8b8b8" },
  num: { color: "#c8c8c8" },
  com: { color: "#6a6a6a", italic: true },
  punct: { color: "#8c8c8c" },
  plain: { color: "#d0d0d0" },
};
