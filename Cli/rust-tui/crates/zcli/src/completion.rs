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

pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec { name: "tools", description: "tool commands", subcommands: &["dump"] },
    CommandSpec { name: "session", description: "session management", subcommands: &["list", "export", "branch", "tree"] },
    CommandSpec { name: "memory", description: "Meridian memory", subcommands: &["search", "add"] },
    CommandSpec { name: "completion", description: "generate shell completion", subcommands: &["bash", "zsh", "fish"] },
    CommandSpec { name: "compact", description: "compact a session", subcommands: &[] },
    CommandSpec { name: "undo", description: "undo the last checkpoint", subcommands: &[] },
    CommandSpec { name: "checkpoint", description: "checkpoint operations", subcommands: &["restore"] },
    CommandSpec { name: "auth", description: "authentication", subcommands: &["login", "logout"] },
    CommandSpec { name: "models", description: "list models", subcommands: &[] },
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

pub fn script(shell: &str, bin: &str) -> Option<String> {
    let top = words(COMMANDS.iter().map(|c| c.name).chain(["--help", "--version"]));
    let flags = words(FLAGS.iter().map(|f| f.name));
    let session = "list export branch tree";
    let memory = "search add";
    let tools = "dump";
    let completion = "bash zsh fish";
    match shell {
        "bash" => Some(format!(
            r#"_{{bin}}_complete() {{
  local cur="${{COMP_WORDS[COMP_CWORD]}}" cmd="${{COMP_WORDS[1]}}"
  if [[ "$COMP_CWORD" -eq 1 ]]; then
    COMPREPLY=($(compgen -W "{top}" -- "$cur")); return
  fi
  case "$cmd" in
    session) COMPREPLY=($(compgen -W "{session} {flags}" -- "$cur"));;
    memory) COMPREPLY=($(compgen -W "{memory} {flags}" -- "$cur"));;
    tools) COMPREPLY=($(compgen -W "{tools} {flags}" -- "$cur"));;
    completion) COMPREPLY=($(compgen -W "{completion}" -- "$cur"));;
    *) COMPREPLY=($(compgen -W "{flags}" -- "$cur"));;
  esac
}}
complete -F _{{bin}}_complete {bin}
"#,
            bin = bin,
            top = top,
            flags = flags,
            session = session,
            memory = memory,
            tools = tools,
            completion = completion
        )),
        "zsh" => {
            let args = FLAGS.iter().map(|f| {
                if f.takes_value {
                    format!("'{}[{}]:value:'", f.name, f.description)
                } else {
                    format!("'{}[{}]'", f.name, f.description)
                }
            }).collect::<Vec<_>>().join(" \\\n  ");
            Some(format!(
                "#compdef {bin}\n_{bin}() {{\n  local -a commands\n  commands=({top})\n  _arguments -C \\\n    '1:command:(({top}))' \\\n    '*:arguments:->args'\n  case $state in\n    args)\n      case $words[2] in\n        session) _arguments '1:subcommand:(({session}))' {args};;\n        memory) _arguments '1:subcommand:(({memory}))' {args};;\n        tools) _arguments '1:subcommand:(({tools}))' {args};;\n        completion) _values 'shell' {completion};;\n        *) _arguments {args};;\n      esac\n      ;;\n  esac\n}}\ncompdef _{bin} {bin}\n",
                bin = bin,
                top = top,
                args = args,
                session = session,
                memory = memory,
                tools = tools,
                completion = completion
            ))
        }
        "fish" => {
            let mut out = format!(
                "complete -c {bin} -f -a '{top}' -d 'Cosmos CLI command'\n",
                bin = bin,
                top = top
            );
            for f in FLAGS {
                out.push_str(&format!(
                    "complete -c {bin} -l {name} -d '{desc}'{value}\n",
                    bin = bin,
                    name = f.name.trim_start_matches('-'),
                    desc = f.description,
                    value = if f.takes_value { " -r" } else { "" }
                ));
            }
            out.push_str(&format!("complete -c {bin} -n '__fish_seen_subcommand_from session' -a '{session}' -d 'session operation'\n", bin = bin, session = session));
            out.push_str(&format!("complete -c {bin} -n '__fish_seen_subcommand_from memory' -a '{memory}' -d 'memory operation'\n", bin = bin, memory = memory));
            out.push_str(&format!("complete -c {bin} -n '__fish_seen_subcommand_from tools' -a '{tools}' -d 'tool operation'\n", bin = bin, tools = tools));
            out.push_str(&format!("complete -c {bin} -n '__fish_seen_subcommand_from completion' -a '{completion}' -d 'shell'\n", bin = bin, completion = completion));
            Some(out)
        }
        _ => None,
    }
}
