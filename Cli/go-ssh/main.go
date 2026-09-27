// zagy — Cosmos Go harness. `zagy -p "task"` headless, bare `zagy` REPL.
package main

import (
	"bufio"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
)

const help = `zagy — Cosmos coding-agent harness (Go)

usage:
  zagy                        interactive session (TUI; line REPL on pipes)
  zagy -p "<task>" [flags]    headless run; -p - reads the task from stdin
  zagy onboarding             replay the first-run setup
  zagy tools dump             canonical tools.json (byte-identical to zcli/zdev)
  zagy session list           sessions in COSMOS_CLI_HOME
  zagy session export <id> [--format md|jsonl]
  zagy session branch <id> --at <event_id>
  zagy session tree <id>
  zagy memory search <q>      search the shared Meridian vault
  zagy memory add <text>      append to the vault (scope: zagy)

flags: --output/--output-format text|json|stream-json · --mode plan|agent|accept-edits · --allow a,b ·
--yolo · --no-sandbox · --model <alias> · --max-turns N · --max-usd F ·
--cwd <dir> · --resume <id> · --continue/-c · --add-dir <path> (repeatable) ·
--system-prompt <text|@file> · --append-system-prompt <text|@file> ·
--allowedTools/--allowed-tools · --approve-plan · --allow-escalation cap,... ·
--sandbox read-only|workspace-write|full · --ask on-request|never|untrusted ·
--autonomous · --autonomous-gate "<cmd>" · --allow-self-mutate · --arena 1
`

func splitList(s string) []string {
	var out []string
	for _, x := range strings.Split(s, ",") {
		if x = strings.TrimSpace(x); x != "" {
			out = append(out, x)
		}
	}
	return out
}

func parse(args []string) (*RunOpts, error) {
	o := &RunOpts{Output: "text", MaxTurns: 16}
	var positional []string
	i := 0
	val := func() (string, error) {
		i++
		if i >= len(args) {
			return "", fmt.Errorf("%s needs a value", args[i-1])
		}
		return args[i], nil
	}
	for i < len(args) {
		a := args[i]
		switch a {
		case "-p", "--prompt":
			v, err := val()
			if err != nil {
				return nil, err
			}
			o.Prompt = &v
		case "--output", "--output-format":
			v, err := val()
			if err != nil {
				return nil, err
			}
			if v == "stream-json" {
				v = "json-stream"
			}
			if v != "text" && v != "json" && v != "json-stream" {
				return nil, fmt.Errorf("bad --output %s", v)
			}
			o.Output = v
		case "--mode":
			v, err := val()
			if err != nil {
				return nil, err
			}
			o.Mode = v
		case "--allow", "--allowedTools", "--allowed-tools":
			v, err := val()
			if err != nil {
				return nil, err
			}
			o.Allow = splitList(v)
		case "--yolo":
			o.Yolo = true
		case "--no-sandbox":
			o.NoSandbox = true
		case "--model":
			v, err := val()
			if err != nil {
				return nil, err
			}
			o.Model = v
		case "--max-turns":
			v, err := val()
			if err != nil {
				return nil, err
			}
			n, err := strconv.Atoi(v)
			if err != nil {
				return nil, fmt.Errorf("bad --max-turns")
			}
			o.MaxTurns = n
		case "--max-usd":
			v, err := val()
			if err != nil {
				return nil, err
			}
			f, err := strconv.ParseFloat(v, 64)
			if err != nil {
				return nil, fmt.Errorf("bad --max-usd")
			}
			o.MaxUSD = f
		case "--cwd":
			v, err := val()
			if err != nil {
				return nil, err
			}
			o.Cwd = v
		case "--resume":
			v, err := val()
			if err != nil {
				return nil, err
			}
			o.Resume = v
		case "--continue", "-c":
			o.Resume = "__latest__"
		case "--add-dir":
			v, err := val()
			if err != nil {
				return nil, err
			}
			o.AddDirs = append(o.AddDirs, v)
		case "--system-prompt":
			v, err := val()
			if err != nil {
				return nil, err
			}
			o.SystemPrompt = v
		case "--append-system-prompt":
			v, err := val()
			if err != nil {
				return nil, err
			}
			o.AppendSystemPrompt = v
		case "--approve-plan":
			o.ApprovePlan = true
		case "--allow-escalation":
			v, err := val()
			if err != nil {
				return nil, err
			}
			o.AllowEscalation = splitList(v)
		case "--sandbox":
			v, err := val()
			if err != nil {
				return nil, err
			}
			o.Sandbox = v
		case "--ask":
			v, err := val()
			if err != nil {
				return nil, err
			}
			o.Ask = v
		case "--autonomous":
			o.Autonomous = true
		case "--autonomous-gate":
			v, err := val()
			if err != nil {
				return nil, err
			}
			o.AutonomousGate = v
		case "--allow-self-mutate":
			o.AllowSelfMutate = true
		case "--arena":
			if _, err := val(); err != nil {
				return nil, err
			}
			o.Arena = true
		case "--self-improve":
			t := true
			o.SelfImprove = &t
		case "--no-self-improve":
			f := false
			o.SelfImprove = &f
		case "--version", "--help", "-h", "-V":
			fmt.Printf("zagy contract v%d\n%s", ContractVersion, help)
			os.Exit(ExitDone)
		default:
			if strings.HasPrefix(a, "-") {
				return nil, fmt.Errorf("unknown flag %s", a)
			}
			positional = append(positional, a)
		}
		i++
	}
	if o.Prompt == nil && len(positional) > 0 {
		p := strings.Join(positional, " ")
		o.Prompt = &p
	}
	return o, nil
}

func emitter(mode string) func(Event) {
	return func(ev Event) {
		switch mode {
		case "json-stream":
			fmt.Println(ev.Line())
		case "json":
			b, _ := json.MarshalIndent(ev, "", "  ")
			fmt.Println(string(b))
		default:
			switch ev.Kind {
			case "assistant":
				if t := dataStr(ev, "text"); t != "" {
					fmt.Println(t)
				}
			case "tool_result":
				c := dataStr(ev, "content")
				if len(c) > 200 {
					c = c[:200]
				}
				fmt.Fprintf(os.Stderr, "  [%s] %s\n", dataStr(ev, "name"), c)
			}
		}
	}
}

func renderTUI(ev Event) {
	d := dataStr
	switch ev.Kind {
	case "assistant":
		if t := d(ev, "text"); t != "" {
			fmt.Println(t)
		}
		for _, c := range anySlice(dataGet(ev, "tool_calls")) {
			cm, _ := c.(map[string]any)
			inp, _ := json.Marshal(cm["input"])
			fmt.Fprintf(os.Stderr, "  → %v %s\n", cm["name"], trunc(string(inp), 120))
		}
	case "tool_result":
		mark := "✓"
		if e, ok := dataGet(ev, "is_error").(bool); ok && e {
			mark = "✗"
		}
		fmt.Fprintf(os.Stderr, "  %s %s: %s\n", mark, d(ev, "name"),
			trunc(d(ev, "content"), 160))
	case "plan":
		fmt.Printf("\n── plan ──\n%s\n──────────\n(/approve to run, or keep refining)\n",
			d(ev, "plan"))
	case "subagent_start":
		fmt.Fprintf(os.Stderr, "  ⎇ subagent: %s\n", d(ev, "prompt"))
	}
}

func runTurns(a *Agent, emit func(Event)) {
	max := a.Opts.MaxTurns
	if max <= 0 {
		max = 32
	}
	for i := 0; i < max; i++ {
		done, err := a.Turn(emit)
		if err != nil {
			fmt.Fprintf(os.Stderr, "turn failed: %v\n", err)
			break
		}
		if done {
			break
		}
		last := a.Messages[len(a.Messages)-1]
		var calls []toolCall
		for _, b := range anySlice(last["content"]) {
			bm, _ := b.(map[string]any)
			if bm["type"] != "tool_use" {
				continue
			}
			inp, _ := bm["input"].(map[string]any)
			calls = append(calls, toolCall{ID: strv(bm, "id"), Name: strv(bm, "name"), Input: inp})
		}
		results := a.ExecToolsBatch(calls, emit)
		for _, r := range results {
			if name, _ := r[0].(string); name == "submit_plan" {
				return
			}
		}
	}
}

func repl(opts *RunOpts) int {
	cwd, _ := os.Getwd()
	a, err := NewAgent(opts, "zagy", cwd)
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		return ExitFailed
	}
	return lineREPL(a, opts)
}

// lineREPL is the plain line-mode session loop — used when the TUI cannot
// start (no TTY on stdin) or when bubbletea init fails mid-launch.
func lineREPL(a *Agent, opts *RunOpts) int {
	fmt.Fprintf(os.Stderr, "zagy · %s · mode %s · session %s\n",
		a.ModelAlias, a.Mode, a.Session.ID[:8])
	if a.Ctx.SandboxDegraded != "" {
		fmt.Fprintf(os.Stderr, "sandbox degraded: %s\n", a.Ctx.SandboxDegraded)
	}
	sc := bufio.NewScanner(os.Stdin)
	a.Asker = func(tool string) bool {
		fmt.Fprintf(os.Stderr, "allow %s? [y/N] ", tool)
		if sc.Scan() {
			t := strings.TrimSpace(strings.ToLower(sc.Text()))
			return t == "y" || t == "yes"
		}
		return false
	}
	emit := renderTUI
	a.Session.append("session_start", map[string]any{
		"model": a.ModelAlias, "mode": a.Mode,
		"sandbox":     map[bool]string{true: "off", false: "appcontainer"}[a.Ctx.NoSandbox],
		"boot_tokens": a.BootTokens})
	if opts.Prompt != nil && *opts.Prompt != "-" {
		a.SendUser(*opts.Prompt, emit)
		runTurns(a, emit)
	}
	for {
		fmt.Fprint(os.Stderr, "› ")
		if !sc.Scan() {
			break
		}
		line := strings.TrimSpace(sc.Text())
		if line == "" {
			continue
		}
		switch line {
		case "/quit", "/exit", "/q":
			a.Session.append("session_end", map[string]string{"reason": "quit"})
			return ExitDone
		case "/plan":
			a.Mode = "plan"
			fmt.Fprintln(os.Stderr, "mode: plan")
		case "/agent":
			a.Mode = "agent"
			fmt.Fprintln(os.Stderr, "mode: agent")
		case "/yolo":
			a.Mode = "yolo"
			fmt.Fprintln(os.Stderr, "mode: yolo")
		case "/accept-edits":
			a.Mode = "accept-edits"
			fmt.Fprintln(os.Stderr, "mode: accept-edits")
		case "/compact":
			a.CompactNow("", emit)
			fmt.Fprintln(os.Stderr, "compacted")
		case "/undo":
			fmt.Fprintln(os.Stderr, a.UndoLast(emit))
		case "/approve":
			a.ApprovePlan(emit)
			runTurns(a, emit)
		case "/session":
			fmt.Fprintf(os.Stderr, "session %s\n", a.Session.ID)
		case "/help":
			fmt.Fprintln(os.Stderr, "commands: /plan /agent /accept-edits /yolo /compact /undo /skillify /approve /onboarding /model /providers /connect /context /usage /session /quit")
		case "/context":
			fmt.Fprintln(os.Stderr, a.contextReport())
		case "/usage":
			fmt.Fprintf(os.Stderr, "tokens · in %d · out %d · Σ $%.4f\n", a.LastUsage.Input, a.LastUsage.Output, a.CumUSD)
		default:
			if strings.HasPrefix(line, "/skillify") {
				name := strings.TrimSpace(strings.TrimPrefix(line, "/skillify"))
				if name == "" {
					name = "session"
				}
				fmt.Fprintln(os.Stderr, a.Skillify(name, emit))
				continue
			}
			if strings.HasPrefix(line, "/compact") {
				focus := strings.TrimSpace(strings.TrimPrefix(line, "/compact"))
				focus = strings.TrimSpace(strings.TrimPrefix(strings.TrimPrefix(focus, "focus:"), "focus"))
				a.CompactNow(focus, emit)
				fmt.Fprintln(os.Stderr, "compacted")
				continue
			}
			a.SendUser(line, emit)
			runTurns(a, emit)
		}
	}
	a.Session.append("session_end", map[string]string{"reason": "quit"})
	return ExitDone
}

func runHeadless(opts *RunOpts, harness, cwd string) int {
	prompt := *opts.Prompt
	if prompt == "-" {
		b, _ := io.ReadAll(os.Stdin)
		prompt = string(b)
	}
	a, err := NewAgent(opts, harness, cwd)
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		return ExitFailed
	}
	defer a.cleanup()
	emit := emitter(opts.Output)
	emit(a.Session.append("session_start", map[string]any{
		"model": a.ModelAlias, "mode": a.Mode,
		"sandbox":     map[bool]string{true: "off", false: "appcontainer"}[a.Ctx.NoSandbox],
		"boot_tokens": a.BootTokens, "resume": optStr(opts.Resume),
		"cwd": filepath.Clean(a.Workspace)}))
	runHooks(a.Cfg, "session_start", "", map[string]any{}, a.Session.ID)
	if strings.HasPrefix(prompt, "/compact") {
		focus := strings.TrimSpace(strings.TrimPrefix(prompt, "/compact"))
		focus = strings.TrimSpace(strings.TrimPrefix(strings.TrimPrefix(focus, "focus:"), "focus"))
		a.CompactNow(focus, emit)
		emit(a.Session.append("session_end", map[string]string{"reason": "compact"}))
		return ExitDone
	}
	if strings.TrimSpace(prompt) == "/undo" {
		fmt.Fprintln(os.Stderr, a.UndoLast(emit))
		emit(a.Session.append("session_end", map[string]string{"reason": "undo"}))
		return ExitDone
	}
	if strings.HasPrefix(prompt, "/skillify") {
		name := strings.TrimSpace(strings.TrimPrefix(prompt, "/skillify"))
		if name == "" {
			name = "session"
		}
		fmt.Fprintln(os.Stderr, a.Skillify(name, emit))
		emit(a.Session.append("session_end", map[string]string{"reason": "skillify"}))
		return ExitDone
	}
	a.SendUser(prompt, emit)
	runHooks(a.Cfg, "user_prompt", "", map[string]string{"text": prompt}, a.Session.ID)
	max := opts.MaxTurns
	if max <= 0 {
		max = 16
	}
	for i := 0; i < max; i++ {
		if opts.MaxUSD > 0 && a.CumUSD >= opts.MaxUSD {
			return ExitBudget
		}
		done, err := a.Turn(emit)
		if err != nil {
			fmt.Fprintln(os.Stderr, err)
			emit(a.Session.append("session_end", map[string]string{"reason": "provider_error"}))
			return ExitProvider
		}
		if done {
			if opts.AutonomousGate != "" {
				var c *exec.Cmd
				if filepath.Separator == '\\' {
					c = exec.Command("cmd", "/C", opts.AutonomousGate)
				} else {
					c = exec.Command("sh", "-lc", opts.AutonomousGate)
				}
				c.Dir = a.Workspace
				out, err := c.CombinedOutput()
				if err != nil {
					a.Messages = append(a.Messages, map[string]any{
						"role":    "user",
						"content": []any{map[string]any{"type": "text", "text": "[autonomous-gate failed]\n" + string(out)}},
					})
					continue
				}
			}
			if a.ImproveEnabled {
				draft := "draft-"
				if len(a.Session.ID) >= 8 {
					draft += a.Session.ID[:8]
				} else {
					draft += a.Session.ID
				}
				if dest, err := skillify(a.Home, draft, a.World, true, a.AllowSelfMutate); err == nil {
					emit(a.Session.append("self_improve", improveEvent("draft_skill", map[string]any{"name": draft, "path": dest, "draft": true})))
				}
			}
			emit(a.Session.append("session_end", map[string]string{"reason": "done"}))
			return ExitDone
		}
		last := a.Messages[len(a.Messages)-1]
		denied := false
		var calls []toolCall
		for _, b := range anySlice(last["content"]) {
			bm, _ := b.(map[string]any)
			if bm["type"] != "tool_use" {
				continue
			}
			inp, _ := bm["input"].(map[string]any)
			calls = append(calls, toolCall{ID: strv(bm, "id"), Name: strv(bm, "name"), Input: inp})
		}
		results := a.ExecToolsBatch(calls, emit)
		for _, r := range results {
			name, _ := r[0].(string)
			text, _ := r[1].(string)
			isErr, _ := r[2].(bool)
			if name == "submit_plan" && !isErr {
				if opts.ApprovePlan {
					a.ApprovePlan(emit)
				} else {
					emit(a.Session.append("session_end",
						map[string]string{"reason": "plan_submitted"}))
					return ExitDone
				}
			}
			if isErr && strings.HasPrefix(text, "permission denied") {
				denied = true
			}
		}
		if denied {
			emit(a.Session.append("session_end",
				map[string]string{"reason": "permission_denied"}))
			return ExitDenied
		}
	}
	emit(a.Session.append("session_end", map[string]string{"reason": "max_turns"}))
	return ExitFailed
}

func optStr(s string) any {
	if s == "" {
		return nil
	}
	return s
}

func modelsCmd(args []string) int {
	cfg := loadConfig(cliHome())
	want := ""
	if len(args) > 0 {
		want = args[0]
	}
	if want != "" && catalogProvider(want) == nil && cfg.Providers[want] == nil {
		fmt.Fprintf(os.Stderr, "unknown provider %q\n", want)
		return ExitUsage
	}
	n := 0
	for _, m := range cfg.availableModels("") {
		if want != "" && m.provider != want {
			continue
		}
		fmt.Printf("%-48s %8d  %s\n", m.alias, m.context, m.provider)
		n++
	}
	if n == 0 {
		if want == "devin" {
			fmt.Fprintln(os.Stderr, "no Devin models yet — /providers → Devin (subscription models; Cosmos runs the tools)")
		} else {
			fmt.Fprintln(os.Stderr, "no models yet")
		}
		return ExitUsage
	}
	return ExitDone
}

func sessionCmd(args []string) int {
	home := cliHome()
	if len(args) == 0 {
		fmt.Fprintln(os.Stderr, "usage: zagy session list|export|branch|tree")
		return ExitUsage
	}
	switch args[0] {
	case "list":
		for _, s := range sessionList(home) {
			b, _ := json.Marshal(s)
			fmt.Println(string(b))
		}
		return ExitDone
	case "export":
		if len(args) < 2 {
			return ExitUsage
		}
		format := "md"
		for i, a := range args {
			if a == "--format" && i+1 < len(args) {
				format = args[i+1]
			}
		}
		out, err := sessionExport(home, args[1], format)
		if err != nil {
			fmt.Fprintln(os.Stderr, err)
			return ExitUsage
		}
		fmt.Print(out)
		return ExitDone
	case "branch":
		if len(args) < 2 {
			return ExitUsage
		}
		at := ""
		for i, a := range args {
			if a == "--at" && i+1 < len(args) {
				at = args[i+1]
			}
		}
		s, err := sessionOpen(home, args[1])
		if err != nil {
			fmt.Fprintln(os.Stderr, err)
			return ExitUsage
		}
		s.branch(at)
		fmt.Printf("branched %s at %s\n", args[1], at)
		return ExitDone
	case "tree":
		if len(args) < 2 {
			return ExitUsage
		}
		s := &Session{File: filepath.Join(sessionsDir(home), args[1]+".jsonl")}
		evs, err := s.events()
		if err != nil {
			fmt.Fprintln(os.Stderr, err)
			return ExitUsage
		}
		byParent := map[string][]Event{}
		for _, e := range evs {
			key := ""
			if e.Parent != nil {
				key = *e.Parent
			}
			byParent[key] = append(byParent[key], e)
		}
		var walk func(parent string, depth int)
		walk = func(parent string, depth int) {
			for _, e := range byParent[parent] {
				fmt.Printf("%s%s [%s] %s\n", strings.Repeat("  ", depth), e.ID, e.Kind, e.TS)
				walk(e.ID, depth+1)
			}
		}
		walk("", 0)
		return ExitDone
	}
	fmt.Fprintln(os.Stderr, "usage: zagy session list|export|branch|tree")
	return ExitUsage
}

func memoryCmd(args []string) int {
	cwd, _ := os.Getwd()
	ctx := NewToolCtx(cwd)
	if len(args) == 0 {
		fmt.Fprintln(os.Stderr, "usage: zagy memory search|add")
		return ExitUsage
	}
	switch args[0] {
	case "search":
		fmt.Println(runTool(ctx, "memory_search",
			map[string]any{"query": strings.Join(args[1:], " ")}))
		return ExitDone
	case "add":
		ty := "map"
		var words []string
		for i := 1; i < len(args); i++ {
			if args[i] == "--type" && i+1 < len(args) {
				i++
				ty = args[i]
			} else {
				words = append(words, args[i])
			}
		}
		fmt.Println(runTool(ctx, "memory_write",
			map[string]any{"type": ty, "text": strings.Join(words, " "), "scope": "zagy"}))
		return ExitDone
	}
	fmt.Fprintln(os.Stderr, "usage: zagy memory search|add")
	return ExitUsage
}

func main() {
	args := os.Args[1:]
	cwd, _ := os.Getwd()
	code := ExitDone
	if len(args) > 0 && args[0] == "completion" {
		shell := "bash"
		if len(args) > 1 {
			shell = args[1]
		}
		if shell != "bash" && shell != "zsh" && shell != "fish" {
			fmt.Fprintln(os.Stderr, "completion shell must be bash, zsh, or fish")
			os.Exit(ExitUsage)
		}
		fmt.Println("_zagy_complete() { COMPREPLY=( $(compgen -W 'completion tools session models memory compact undo checkpoint --help --version --output-format --allowedTools --add-dir --system-prompt --append-system-prompt --continue' -- \"${COMP_WORDS[COMP_CWORD]}\") ); }\ncomplete -F _zagy_complete zagy")
		os.Exit(ExitDone)
	}
	if len(args) > 0 && args[0] == "tools" {
		if len(args) > 1 && args[1] == "dump" {
			fmt.Print(ToolsJSON)
			os.Exit(ExitDone)
		}
		fmt.Fprintln(os.Stderr, "usage: zagy tools dump")
		os.Exit(ExitUsage)
	}
	if len(args) > 0 && args[0] == "session" {
		os.Exit(sessionCmd(args[1:]))
	}
	if len(args) > 0 && args[0] == "models" {
		os.Exit(modelsCmd(args[1:]))
	}
	if len(args) > 0 && args[0] == "memory" {
		os.Exit(memoryCmd(args[1:]))
	}
	if len(args) > 0 && args[0] == "acp" {
		os.Exit(acpRun())
	}
	if len(args) > 0 && (args[0] == "onboarding" || args[0] == "onboard") {
		os.Exit(runOnboarding())
	}
	if len(args) > 0 && (args[0] == "compact" || args[0] == "undo" || args[0] == "skillify" || args[0] == "checkpoint") {
		flag := func(n string) string {
			for i, a := range args {
				if a == n && i+1 < len(args) {
					return args[i+1]
				}
			}
			return ""
		}
		o := &RunOpts{Output: "text", Model: "fixture"}
		switch args[0] {
		case "compact":
			sid := flag("--session")
			if sid == "" {
				fmt.Fprintln(os.Stderr, "usage: zagy compact --session <id> [--focus …]")
				os.Exit(ExitUsage)
			}
			o.Resume = sid
			focus := flag("--focus")
			p := "/compact"
			if focus != "" {
				p = "/compact focus: " + focus
			}
			o.Prompt = &p
			os.Exit(runHeadless(o, "zagy", cwd))
		case "undo", "checkpoint":
			sid := flag("--session")
			if sid == "" {
				if list := sessionList(cliHome()); len(list) > 0 {
					if id, ok := list[len(list)-1]["id"].(string); ok {
						sid = id
					}
				}
			}
			if sid == "" {
				fmt.Fprintln(os.Stderr, "usage: zagy undo --session <id>")
				os.Exit(ExitUsage)
			}
			o.Resume = sid
			p := "/undo"
			o.Prompt = &p
			os.Exit(runHeadless(o, "zagy", cwd))
		case "skillify":
			name := "session"
			if len(args) > 1 && !strings.HasPrefix(args[1], "-") {
				name = args[1]
			}
			p := "/skillify " + name
			o.Prompt = &p
			o.Resume = flag("--session")
			os.Exit(runHeadless(o, "zagy", cwd))
		}
	}
	opts, err := parse(args)
	if err != nil {
		fmt.Fprintf(os.Stderr, "%v\n%s", err, help)
		os.Exit(ExitUsage)
	}
	if opts.Prompt != nil {
		code = runHeadless(opts, "zagy", cwd)
	} else {
		code = runTUI(opts, cwd)
	}
	os.Exit(code)
}
