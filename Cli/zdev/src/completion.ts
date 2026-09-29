export type CompletionSpec = {
  name: string;
  description: string;
  subcommands?: string[];
};

export type FlagSpec = {
  name: string;
  description: string;
  takesValue?: boolean;
};

export const COMMANDS: CompletionSpec[] = [
  { name: "tools", description: "tool commands", subcommands: ["dump"] },
  { name: "session", description: "session management", subcommands: ["list", "export", "branch", "tree"] },
  { name: "memory", description: "Meridian memory", subcommands: ["search", "add"] },
  { name: "completion", description: "generate shell completion", subcommands: ["bash", "zsh", "fish"] },
  { name: "compact", description: "compact a session" },
  { name: "undo", description: "undo the last checkpoint" },
  { name: "checkpoint", description: "checkpoint operations", subcommands: ["restore"] },
  { name: "models", description: "list models" },
  { name: "onboarding", description: "run onboarding" },
];

export const FLAGS: FlagSpec[] = [
  ["prompt", "headless prompt", true],
  ["output-format", "text, json, or stream-json", true],
  ["mode", "plan or agent", true],
  ["allow", "allowed tools", true],
  ["allowedTools", "alias for --allow", true],
  ["allowed-tools", "alias for --allow", true],
  ["yolo", "allow all tools"],
  ["no-sandbox", "disable bash sandbox"],
  ["model", "model alias", true],
  ["max-turns", "turn limit", true],
  ["max-usd", "budget cap", true],
  ["cwd", "workspace root", true],
  ["resume", "resume session id", true],
  ["continue", "resume newest session"],
  ["add-dir", "additional sandbox root", true],
  ["system-prompt", "replace system prompt", true],
  ["append-system-prompt", "append system prompt", true],
  ["approve-plan", "approve a plan"],
  ["allow-escalation", "escalation capabilities", true],
  ["sandbox", "sandbox mode", true],
  ["ask", "permission mode", true],
  ["autonomous", "autonomous mode"],
  ["autonomous-gate", "autonomous gate command", true],
  ["allow-self-mutate", "allow self mutation"],
  ["arena", "arena mode", true],
  ["self-improve", "enable self improvement"],
  ["no-self-improve", "disable self improvement"],
  ["help", "show help"],
  ["version", "show version"],
].map(([name, description, takesValue]) => ({ name, description, takesValue }));

export function completionScript(shell: string, bin: string): string | null {
  const top = [...COMMANDS.map((c) => c.name), "--help", "--version"].join(" ");
  const flags = FLAGS.map((f) => `--${f.name}`).join(" ");
  const sub = (name: string) => COMMANDS.find((c) => c.name === name)?.subcommands?.join(" ") ?? "";
  if (shell === "bash") {
    return `_${bin}_complete() {
  local cur="\${COMP_WORDS[COMP_CWORD]}" cmd="\${COMP_WORDS[1]}"
  if [[ "\${COMP_CWORD}" -eq 1 ]]; then
    COMPREPLY=( $(compgen -W "${top}" -- "$cur") ); return
  fi
  case "$cmd" in
    session) COMPREPLY=( $(compgen -W "${sub("session")} ${flags}" -- "$cur") );;
    memory) COMPREPLY=( $(compgen -W "${sub("memory")} ${flags}" -- "$cur") );;
    tools) COMPREPLY=( $(compgen -W "${sub("tools")} ${flags}" -- "$cur") );;
    completion) COMPREPLY=( $(compgen -W "${sub("completion")}" -- "$cur") );;
    *) COMPREPLY=( $(compgen -W "${flags}" -- "$cur") );;
  esac
}
complete -F _${bin}_complete ${bin}
`;
  }
  if (shell === "zsh") {
    const args = FLAGS.map((f) => `'--${f.name}[${f.description}]${f.takesValue ? ":value:" : ""}'`).join(" \\\n    ");
    return `#compdef ${bin}
_${bin}() {
  _arguments -C \\
    '1:command:(${top})' \\
    '*:arguments:->args'
  case $state in
    args)
      case $words[2] in
        session) _arguments '1:subcommand:(${sub("session")})' ${args};;
        memory) _arguments '1:subcommand:(${sub("memory")})' ${args};;
        tools) _arguments '1:subcommand:(${sub("tools")})' ${args};;
        completion) _values 'shell' ${sub("completion")};;
        *) _arguments ${args};;
      esac
      ;;
  esac
}
compdef _${bin} ${bin}
`;
  }
  if (shell === "fish") {
    const lines = [`complete -c ${bin} -f -a '${top}' -d 'Cosmos CLI command'`];
    for (const f of FLAGS) {
      lines.push(`complete -c ${bin} -l ${f.name} -d '${f.description}'${f.takesValue ? " -r" : ""}`);
    }
    lines.push(`complete -c ${bin} -n '__fish_seen_subcommand_from session' -a '${sub("session")}' -d 'session operation'`);
    lines.push(`complete -c ${bin} -n '__fish_seen_subcommand_from memory' -a '${sub("memory")}' -d 'memory operation'`);
    lines.push(`complete -c ${bin} -n '__fish_seen_subcommand_from tools' -a '${sub("tools")}' -d 'tool operation'`);
    lines.push(`complete -c ${bin} -n '__fish_seen_subcommand_from completion' -a '${sub("completion")}' -d 'shell'`);
    return `${lines.join("\n")}\n`;
  }
  return null;
}
