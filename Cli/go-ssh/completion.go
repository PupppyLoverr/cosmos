package main

import "strings"

type completionCommand struct {
	name        string
	description string
	subcommands []string
}

type completionFlag struct {
	name        string
	description string
	takesValue  bool
}

var completionCommands = []completionCommand{
	{"tools", "tool commands", []string{"dump"}},
	{"session", "session management", []string{"list", "export", "branch", "tree"}},
	{"memory", "Meridian memory", []string{"search", "add"}},
	{"completion", "generate shell completion", []string{"bash", "zsh", "fish"}},
	{"compact", "compact a session", nil},
	{"undo", "undo the last checkpoint", nil},
	{"checkpoint", "checkpoint operations", []string{"restore"}},
	{"models", "list models", nil},
	{"onboarding", "run onboarding", nil},
}

var completionFlags = []completionFlag{
	{"prompt", "headless prompt", true},
	{"output-format", "text, json, or stream-json", true},
	{"mode", "plan or agent", true},
	{"allow", "allowed tools", true},
	{"allowedTools", "alias for --allow", true},
	{"allowed-tools", "alias for --allow", true},
	{"yolo", "allow all tools", false},
	{"no-sandbox", "disable bash sandbox", false},
	{"model", "model alias", true},
	{"max-turns", "turn limit", true},
	{"max-usd", "budget cap", true},
	{"cwd", "workspace root", true},
	{"resume", "resume session id", true},
	{"continue", "resume newest session", false},
	{"add-dir", "additional sandbox root", true},
	{"system-prompt", "replace system prompt", true},
	{"append-system-prompt", "append system prompt", true},
	{"approve-plan", "approve a plan", false},
	{"allow-escalation", "escalation capabilities", true},
	{"sandbox", "sandbox mode", true},
	{"ask", "permission mode", true},
	{"autonomous", "autonomous mode", false},
	{"autonomous-gate", "autonomous gate command", true},
	{"allow-self-mutate", "allow self mutation", false},
	{"arena", "arena mode", true},
	{"self-improve", "enable self improvement", false},
	{"no-self-improve", "disable self improvement", false},
	{"help", "show help", false},
	{"version", "show version", false},
}

func completionScript(shell, bin string) string {
	var top []string
	for _, c := range completionCommands {
		top = append(top, c.name)
	}
	top = append(top, "--help", "--version")
	var flags []string
	for _, f := range completionFlags {
		flags = append(flags, "--"+f.name)
	}
	sub := func(name string) string {
		for _, c := range completionCommands {
			if c.name == name {
				return strings.Join(c.subcommands, " ")
			}
		}
		return ""
	}
	switch shell {
	case "bash":
		return `_` + bin + `_complete() {
  local cur="${COMP_WORDS[COMP_CWORD]}" cmd="${COMP_WORDS[1]}"
  if [[ "${COMP_CWORD}" -eq 1 ]]; then
    COMPREPLY=($(compgen -W "` + strings.Join(top, " ") + `" -- "$cur")); return
  fi
  case "$cmd" in
    session) COMPREPLY=($(compgen -W "` + sub("session") + " " + strings.Join(flags, " ") + `" -- "$cur"));;
    memory) COMPREPLY=($(compgen -W "` + sub("memory") + " " + strings.Join(flags, " ") + `" -- "$cur"));;
    tools) COMPREPLY=($(compgen -W "` + sub("tools") + " " + strings.Join(flags, " ") + `" -- "$cur"));;
    completion) COMPREPLY=($(compgen -W "` + sub("completion") + `" -- "$cur"));;
    *) COMPREPLY=($(compgen -W "` + strings.Join(flags, " ") + `" -- "$cur"));;
  esac
}
complete -F _` + bin + `_complete ` + bin + `
`
	case "zsh":
		var args []string
		for _, f := range completionFlags {
			value := ""
			if f.takesValue {
				value = ":value:"
			}
			args = append(args, "'--"+f.name+"["+f.description+"]"+value+"'")
		}
		return "#compdef " + bin + `
_` + bin + `() {
  _arguments -C \
    '1:command:(` + strings.Join(top, " ") + `)' \
    '*:arguments:->args'
  case $state in
    args)
      case $words[2] in
        session) _arguments '1:subcommand:(` + sub("session") + `)' ` + strings.Join(args, " ") + `;;
        memory) _arguments '1:subcommand:(` + sub("memory") + `)' ` + strings.Join(args, " ") + `;;
        tools) _arguments '1:subcommand:(` + sub("tools") + `)' ` + strings.Join(args, " ") + `;;
        completion) _values 'shell' ` + sub("completion") + `;;
        *) _arguments ` + strings.Join(args, " ") + `;;
      esac
      ;;
  esac
}
compdef _` + bin + ` ` + bin + `
`
	case "fish":
		lines := []string{"complete -c " + bin + " -f -a '" + strings.Join(top, " ") + "' -d 'Cosmos CLI command'"}
		for _, f := range completionFlags {
			value := ""
			if f.takesValue {
				value = " -r"
			}
			lines = append(lines, "complete -c "+bin+" -l "+f.name+" -d '"+f.description+"'"+value)
		}
		lines = append(lines,
			"complete -c "+bin+" -n '__fish_seen_subcommand_from session' -a '"+sub("session")+"' -d 'session operation'",
			"complete -c "+bin+" -n '__fish_seen_subcommand_from memory' -a '"+sub("memory")+"' -d 'memory operation'",
			"complete -c "+bin+" -n '__fish_seen_subcommand_from tools' -a '"+sub("tools")+"' -d 'tool operation'",
			"complete -c "+bin+" -n '__fish_seen_subcommand_from completion' -a '"+sub("completion")+"' -d 'shell'")
		return strings.Join(lines, "\n") + "\n"
	default:
		return ""
	}
}
