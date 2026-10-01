//! `cosmostui completion <shell>` — completion scripts generated from one table
//! of commands and flags, so every shell offers the same words.

pub struct CommandSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub subcommands: &'static [&'static str],
}

pub struct FlagSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub takes_value: bool,
}

pub const SHELLS: &[&str] = &["bash", "zsh", "fish", "powershell"];

pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec { name: "tools", description: "tool commands", subcommands: &["dump"] },
    CommandSpec { name: "session", description: "session management", subcommands: &["list", "export", "branch", "tree"] },
    CommandSpec { name: "memory", description: "local memory", subcommands: &["search", "add"] },
    CommandSpec { name: "completion", description: "generate shell completion", subcommands: SHELLS },
    CommandSpec { name: "compact", description: "compact a session", subcommands: &[] },
    CommandSpec { name: "undo", description: "undo the last checkpoint", subcommands: &[] },
    CommandSpec { name: "review", description: "self-improve pass over a session", subcommands: &[] },
    CommandSpec { name: "checkpoint", description: "checkpoint operations", subcommands: &["restore"] },
    CommandSpec { name: "auth", description: "keys and sign-ins", subcommands: &["login", "logout"] },
    CommandSpec { name: "models", description: "list models", subcommands: &[] },
    CommandSpec { name: "acp", description: "serve the Agent Client Protocol on stdio", subcommands: &[] },
    CommandSpec { name: "onboarding", description: "run onboarding", subcommands: &[] },
];

pub const FLAGS: &[FlagSpec] = &[
    FlagSpec { name: "--prompt", description: "headless prompt", takes_value: true },
    FlagSpec { name: "--output-format", description: "text, json, or stream-json", takes_value: true },
    FlagSpec { name: "--mode", description: "plan or agent", takes_value: true },
    FlagSpec { name: "--allow", description: "allowed tools", takes_value: true },
    FlagSpec { name: "--allowedTools", description: "alias for --allow", takes_value: true },
    FlagSpec { name: "--allowed-tools", description: "alias for --allow", takes_value: true },
    FlagSpec { name: "--yolo", description: "allow all tools", takes_value: false },
    FlagSpec { name: "--no-sandbox", description: "disable bash sandbox", takes_value: false },
    FlagSpec { name: "--model", description: "model alias", takes_value: true },
    FlagSpec { name: "--max-turns", description: "turn limit", takes_value: true },
    FlagSpec { name: "--max-usd", description: "budget cap", takes_value: true },
    FlagSpec { name: "--cwd", description: "workspace root", takes_value: true },
    FlagSpec { name: "--resume", description: "resume session id", takes_value: true },
    FlagSpec { name: "--continue", description: "resume newest session", takes_value: false },
    FlagSpec { name: "--add-dir", description: "additional sandbox root", takes_value: true },
    FlagSpec { name: "--system-prompt", description: "replace system prompt", takes_value: true },
    FlagSpec { name: "--append-system-prompt", description: "append system prompt", takes_value: true },
    FlagSpec { name: "--approve-plan", description: "approve a plan", takes_value: false },
    FlagSpec { name: "--allow-escalation", description: "escalation capabilities", takes_value: true },
    FlagSpec { name: "--sandbox", description: "sandbox mode", takes_value: true },
    FlagSpec { name: "--ask", description: "permission mode", takes_value: true },
    FlagSpec { name: "--autonomous", description: "autonomous mode", takes_value: false },
    FlagSpec { name: "--autonomous-gate", description: "autonomous gate command", takes_value: true },
    FlagSpec { name: "--allow-self-mutate", description: "allow self mutation", takes_value: false },
    FlagSpec { name: "--arena", description: "arena mode", takes_value: true },
    FlagSpec { name: "--self-improve", description: "enable self improvement", takes_value: false },
    FlagSpec { name: "--no-self-improve", description: "disable self improvement", takes_value: false },
    FlagSpec { name: "--help", description: "show help", takes_value: false },
    FlagSpec { name: "--version", description: "show version", takes_value: false },
];

fn words(items: impl Iterator<Item = &'static str>) -> String {
    items.collect::<Vec<_>>().join(" ")
}

fn top() -> String {
    words(COMMANDS.iter().map(|c| c.name).chain(["--help", "--version"]))
}

fn flags() -> String {
    words(FLAGS.iter().map(|f| f.name))
}

fn with_subs() -> impl Iterator<Item = &'static CommandSpec> {
    COMMANDS.iter().filter(|c| !c.subcommands.is_empty())
}

pub fn script(shell: &str, bin: &str) -> Option<String> {
    let fname = bin.replace('-', "_");
    match shell {
        "bash" => {
            let cases: String = with_subs()
                .map(|c| format!("    {}) COMPREPLY=($(compgen -W \"{} {}\" -- \"$cur\"));;\n", c.name, c.subcommands.join(" "), flags()))
                .collect();
            Some(format!(
                "_{fname}_complete() {{\n  local cur=\"${{COMP_WORDS[COMP_CWORD]}}\" cmd=\"${{COMP_WORDS[1]}}\"\n  if [[ \"$COMP_CWORD\" -eq 1 ]]; then\n    COMPREPLY=($(compgen -W \"{top}\" -- \"$cur\")); return\n  fi\n  case \"$cmd\" in\n{cases}    *) COMPREPLY=($(compgen -W \"{flags}\" -- \"$cur\"));;\n  esac\n}}\ncomplete -F _{fname}_complete {bin}\n",
                top = top(),
                flags = flags(),
            ))
        }
        "zsh" => {
            let args = FLAGS
                .iter()
                .map(|f| {
                    if f.takes_value {
                        format!("'{}[{}]:value:'", f.name, f.description)
                    } else {
                        format!("'{}[{}]'", f.name, f.description)
                    }
                })
                .collect::<Vec<_>>()
                .join(" \\\n  ");
            let cmds = COMMANDS.iter().map(|c| format!("'{}:{}'", c.name, c.description)).collect::<Vec<_>>().join(" ");
            let cases: String = with_subs()
                .map(|c| format!("        {}) _arguments '1:subcommand:(({}))' {args};;\n", c.name, c.subcommands.join(" ")))
                .collect();
            Some(format!(
                "#compdef {bin}\n_{fname}() {{\n  local -a commands\n  commands=({cmds})\n  _arguments -C \\\n    '1:command:->cmds' \\\n    '*:arguments:->args'\n  case $state in\n    cmds) _describe 'command' commands;;\n    args)\n      case $words[2] in\n{cases}        *) _arguments {args};;\n      esac\n      ;;\n  esac\n}}\ncompdef _{fname} {bin}\n"
            ))
        }
        "fish" => {
            let mut out = String::new();
            for c in COMMANDS {
                out.push_str(&format!("complete -c {bin} -f -n '__fish_use_subcommand' -a '{}' -d '{}'\n", c.name, c.description));
            }
            for f in FLAGS {
                out.push_str(&format!(
                    "complete -c {bin} -l {} -d '{}'{}\n",
                    f.name.trim_start_matches('-'),
                    f.description,
                    if f.takes_value { " -r" } else { "" }
                ));
            }
            for c in with_subs() {
                out.push_str(&format!(
                    "complete -c {bin} -f -n '__fish_seen_subcommand_from {}' -a '{}'\n",
                    c.name,
                    c.subcommands.join(" ")
                ));
            }
            Some(out)
        }
        "powershell" | "pwsh" => {
            let cmds = COMMANDS.iter().map(|c| format!("'{}'", c.name)).collect::<Vec<_>>().join(",");
            let subs = with_subs()
                .map(|c| format!("'{}'=@({})", c.name, c.subcommands.iter().map(|s| format!("'{s}'")).collect::<Vec<_>>().join(",")))
                .collect::<Vec<_>>()
                .join(";");
            let fl = FLAGS.iter().map(|f| format!("'{}'", f.name)).collect::<Vec<_>>().join(",");
            Some(format!(
                "Register-ArgumentCompleter -Native -CommandName {bin} -ScriptBlock {{\n  param($word, $ast, $cursor)\n  $cmds = @({cmds})\n  $subs = @{{{subs}}}\n  $flags = @({fl})\n  $parts = $ast.CommandElements | ForEach-Object {{ $_.ToString() }}\n  $pool = if ($parts.Count -le 1 -or ($parts.Count -eq 2 -and $word)) {{ $cmds + $flags }} elseif ($subs.ContainsKey($parts[1])) {{ $subs[$parts[1]] + $flags }} else {{ $flags }}\n  $pool | Where-Object {{ $_ -like \"$word*\" }} | ForEach-Object {{\n    [System.Management.Automation.CompletionResult]::new($_, $_, 'ParameterValue', $_)\n  }}\n}}\n"
            ))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shell_names_the_binary() {
        for sh in SHELLS {
            let s = script(sh, "cosmostui").unwrap();
            assert!(s.contains("cosmostui"), "{sh}");
            assert!(!s.contains("{bin}") && !s.contains("{fname}"), "{sh}: unrendered placeholder\n{s}");
            assert!(s.contains("session") && s.contains("model"), "{sh}");
        }
        assert!(script("bash", "cosmostui").unwrap().contains("_cosmostui_complete() {"));
        assert!(script("tcsh", "cosmostui").is_none());
    }
}
