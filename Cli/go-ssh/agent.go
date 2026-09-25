// zcore — agent loop, permissions, compaction, hooks (contract §3-§9).
package main

import (
	"encoding/json"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
)

type Agent struct {
	Opts        *RunOpts
	Home        string
	Cfg         *Config
	Workspace   string
	Provider    ProviderCfg
	ModelAlias  string
	Mode        string
	Ctx         *ToolCtx
	Session     *Session
	Messages    []map[string]any
	System      []string
	Tools       []any
	Allowed     []string
	CumUSD      float64
	LastUsage   Usage
	BootTokens  int
	Asker       func(tool string) bool
	PendingPlan string
	// LogW receives the per-turn usage line; io.Discard silences it (TUI).
	LogW io.Writer
	// task{worktree:true} checkouts — removed on cleanup only if clean (§10)
	Worktrees [][2]string
}

func systemSnippet() string {
	// CONTRACT.md sits two levels above go-ssh/ (repo layout), or next to the exe.
	for _, p := range []string{
		filepath.Join("..", "CONTRACT.md"),
		"CONTRACT.md",
	} {
		data, err := os.ReadFile(p)
		if err != nil {
			continue
		}
		text := string(data)
		s := strings.Index(text, "<!-- BEGIN SYSTEM -->")
		e := strings.Index(text, "<!-- END SYSTEM -->")
		if s >= 0 && e > s {
			return strings.TrimSpace(text[s+len("<!-- BEGIN SYSTEM -->") : e])
		}
	}
	return "You are Cosmos, a coding agent running in the operator's terminal. " +
		"You work through tools."
}

func bootContext(workspace string) (string, int) {
	mer := findBin([]string{"meridian.exe", "meridian"}, "MERIDIAN_BIN")
	if mer == "" {
		return systemSnippet(), 0
	}
	c := exec.Command(mer, "boot", "--repo", workspace)
	out, err := c.Output()
	if err != nil {
		return systemSnippet(), 0
	}
	var v map[string]any
	if json.Unmarshal(out, &v) != nil {
		return systemSnippet(), 0
	}
	return strv(v, "context"), int(numv(v, "tokens"))
}

func runHooks(cfg *Config, event, tool string, payload any, session string) (string, bool) {
	ev, _ := json.Marshal(map[string]any{
		"event": event, "tool": tool, "data": payload, "session": session})
	for _, h := range cfg.Hooks {
		if h.Event != event {
			continue
		}
		if h.Matcher != "" && h.Matcher != "*" && h.Matcher != tool {
			continue
		}
		c := exec.Command("cmd", "/c", h.Command)
		c.Stdin = strings.NewReader(string(ev))
		out, err := c.Output()
		if ee, ok := err.(*exec.ExitError); ok {
			if ee.ExitCode() == 2 {
				return string(ee.Stderr), true
			}
		}
		_ = out
	}
	return "", false
}

func NewAgent(opts *RunOpts, harness, cwd string) (*Agent, error) {
	home := cliHome()
	cfg := loadConfig(home)
	ws := opts.Cwd
	if ws == "" {
		ws = cwd
	}
	alias := opts.Model
	if alias == "" {
		alias = cfg.DefaultModel
	}
	if alias == "" {
		alias = "fixture"
	}
	pcfg, ok := cfg.providerFor(alias)
	if !ok {
		pcfg = ProviderCfg{Kind: "fixture", Model: "fixture", Context: 128000,
			Prices: map[string]float64{}}
	}
	mode := opts.Mode
	if opts.Yolo {
		mode = "yolo"
	}
	if mode == "" {
		mode = "agent"
	}
	ctx := NewToolCtx(ws)
	ctx.LargeReadLines = cfg.LargeRead
	ctx.NoSandbox = opts.NoSandbox
	ctx.Escalations = opts.AllowEscalation
	for _, e := range opts.AllowEscalation {
		if e == "network" {
			ctx.NetAllow = true
		}
	}
	a := &Agent{
		Opts: opts, Home: home, Cfg: cfg, Workspace: ws,
		Provider: pcfg, ModelAlias: alias + " (" + pcfg.Model + ")",
		Mode: mode, Ctx: ctx, Allowed: cfg.allowedHosts(),
		LogW: os.Stderr,
	}
	if opts.Resume != "" {
		s, err := sessionOpen(home, opts.Resume)
		if err != nil {
			return nil, err
		}
		s.Harness = harness
		a.Session = s
		a.Messages = s.replay()
	} else {
		a.Session = sessionCreate(home, harness)
	}
	boot, tokens := bootContext(ws)
	a.BootTokens = tokens
	if strings.Contains(boot, "You are Cosmos") {
		a.System = []string{boot}
	} else {
		a.System = []string{systemSnippet(), boot}
	}
	var specs []map[string]any
	json.Unmarshal([]byte(ToolsJSON), &specs)
	for _, s := range specs {
		a.Tools = append(a.Tools, s)
	}
	return a, nil
}

func (a *Agent) permitted(name string) string {
	if a.Mode == "yolo" {
		return "allow"
	}
	if a.Mode == "plan" && !PlanTools[name] {
		return "deny"
	}
	for _, t := range a.Opts.Allow {
		if t == name {
			return "allow"
		}
	}
	for _, t := range a.Cfg.PermAllow {
		if t == name {
			return "allow"
		}
	}
	for _, t := range a.Cfg.PermAsk {
		if t == name {
			return "ask"
		}
	}
	return "deny"
}

func (a *Agent) SendUser(text string, emit func(Event)) {
	emit(a.Session.append("user", map[string]string{"text": text}))
	a.Messages = append(a.Messages, map[string]any{
		"role":    "user",
		"content": []any{map[string]any{"type": "text", "text": text}},
	})
}

func (a *Agent) ApprovePlan(emit func(Event)) {
	if a.PendingPlan == "" {
		return
	}
	plan := a.PendingPlan
	a.PendingPlan = ""
	a.Mode = "agent"
	emit(a.Session.append("mode", map[string]string{"from": "plan", "to": "agent"}))
	a.Messages = append(a.Messages, map[string]any{
		"role":    "user",
		"content": []any{map[string]any{"type": "text", "text": "[approved plan]\n" + plan}},
	})
}

func (a *Agent) cost(u Usage) *float64 {
	var sum float64
	for _, k := range []string{"input", "cache_read", "cache_write", "output"} {
		sum += a.Provider.Prices[k]
	}
	if sum == 0 {
		return nil
	}
	c := (float64(u.Input)*a.Provider.Prices["input"] +
		float64(u.CacheRead)*a.Provider.Prices["cache_read"] +
		float64(u.CacheWrite)*a.Provider.Prices["cache_write"] +
		float64(u.Output)*a.Provider.Prices["output"]) / 1e6
	return &c
}

// Turn runs one provider completion. Returns true when no tool calls.
func (a *Agent) Turn(emit func(Event)) (bool, error) {
	a.maybeCompact(emit)
	var text strings.Builder
	var calls []map[string]any
	var usage Usage
	var provErr error
	req := map[string]any{
		"system":     a.System,
		"tools":      a.Tools,
		"messages":   a.Messages,
		"session_id": a.Session.ID,
		"max_tokens": 4096,
	}
	err := complete(a.Provider, req, func(c map[string]any) {
		switch c["type"] {
		case "text":
			if t, ok := c["text"].(string); ok {
				text.WriteString(t)
			}
		case "tool_use":
			calls = append(calls, c)
		case "done":
			if u, ok := c["usage"].(Usage); ok {
				usage.Input += u.Input
				usage.CacheRead += u.CacheRead
				usage.CacheWrite += u.CacheWrite
				usage.Output += u.Output
			}
		case "err":
			if t, ok := c["text"].(string); ok {
				provErr = fmt.Errorf("%s", t)
			}
		}
	}, a.Allowed)
	if err != nil {
		return false, err
	}
	if provErr != nil {
		return false, provErr
	}
	a.LastUsage = usage
	cost := a.cost(usage)
	if cost != nil {
		a.CumUSD += *cost
	}
	tc := []any{}
	for _, c := range calls {
		tc = append(tc, map[string]any{"id": c["id"], "name": c["name"], "input": c["input"]})
	}
	emit(a.Session.append("assistant", map[string]any{
		"text": text.String(), "tool_calls": tc}))
	var usd any
	if cost != nil {
		usd = *cost
	}
	emit(a.Session.append("usage", map[string]any{
		"input": usage.Input, "cache_read": usage.CacheRead,
		"cache_write": usage.CacheWrite, "output": usage.Output,
		"usd": usd, "cum_usd": a.CumUSD}))
	costStr := "$?"
	if cost != nil {
		costStr = fmt.Sprintf("$%.4f", *cost)
	}
	w := a.LogW
	if w == nil {
		w = os.Stderr
	}
	fmt.Fprintf(w, "turn · in %d · cache_r %d · cache_w %d · out %d · %s · Σ $%.4f · ctx n/a\n",
		usage.Input, usage.CacheRead, usage.CacheWrite, usage.Output, costStr, a.CumUSD)
	var blocks []any
	if text.Len() > 0 {
		blocks = append(blocks, map[string]any{"type": "text", "text": text.String()})
	}
	for _, c := range calls {
		blocks = append(blocks, map[string]any{"type": "tool_use",
			"id": c["id"], "name": c["name"], "input": c["input"]})
	}
	a.Messages = append(a.Messages, map[string]any{"role": "assistant", "content": blocks})
	return len(calls) == 0, nil
}

func (a *Agent) ExecTool(id, name string, inp map[string]any, emit func(Event)) (string, bool) {
	a.Session.append("tool_call", map[string]any{"id": id, "name": name, "input": inp})
	var text string
	var isErr bool
	if blocked, was := runHooks(a.Cfg, "pre_tool", name, inp, a.Session.ID); was {
		text, isErr = blocked, true
	} else {
		switch a.permitted(name) {
		case "deny":
			text = fmt.Sprintf("permission denied: %s is not allowed in %s mode", name, a.Mode)
			isErr = true
		case "ask":
			a.Session.append("permission_request", map[string]any{"tool": name, "input": inp})
			granted := a.Asker != nil && a.Asker(name)
			emit(a.Session.append("permission_decision", map[string]any{
				"tool": name, "input": inp, "granted": granted}))
			if granted {
				text, isErr = a.dispatch(name, inp, emit)
			} else {
				text = fmt.Sprintf("permission denied: %s requires operator approval", name)
				isErr = true
			}
		default:
			text, isErr = a.dispatch(name, inp, emit)
		}
	}
	emit(a.Session.append("tool_result", map[string]any{
		"tool_use_id": id, "name": name, "content": text, "is_error": isErr}))
	runHooks(a.Cfg, "post_tool", name, map[string]any{"input": inp, "is_error": isErr}, a.Session.ID)
	a.Messages = append(a.Messages, map[string]any{
		"role": "user",
		"content": []any{map[string]any{
			"type": "tool_result", "tool_use_id": id,
			"content": text, "is_error": isErr}},
	})
	return text, isErr
}

func (a *Agent) dispatch(name string, inp map[string]any, emit func(Event)) (string, bool) {
	switch name {
	case "submit_plan":
		plan := strv(inp, "plan")
		a.PendingPlan = plan
		emit(a.Session.append("plan", map[string]string{"plan": plan}))
		return fmt.Sprintf("plan submitted (%d chars)", len(plan)), false
	case "request_escalation":
		cap := strv(inp, "capability")
		granted := false
		for _, e := range a.Ctx.Escalations {
			if e == cap {
				granted = true
			}
		}
		emit(a.Session.append("permission_decision", map[string]any{
			"tool": "request_escalation", "capability": cap, "granted": granted}))
		if granted {
			if cap == "network" {
				a.Ctx.NetAllow = true
			}
			return "escalation granted: " + cap, false
		}
		return fmt.Sprintf("escalation denied: %s — rerun with --allow-escalation %s", cap, cap), true
	case "task":
		wt, _ := inp["worktree"].(bool)
		return a.subagent(strv(inp, "prompt"), wt, emit), false
	}
	out := runTool(a.Ctx, name, inp)
	return out, strings.HasPrefix(out, "ERROR")
}

// addWorktree runs `git worktree add COSMOS_CLI_HOME\worktrees\<session>\<child>`
// on a `cosmos-task-<child>` branch. Returns the worktree path.
func (a *Agent) addWorktree(childID string) (string, error) {
	wt := filepath.Join(a.Home, "worktrees", a.Session.ID, childID)
	branch := "cosmos-task-" + childID
	cmd := exec.Command("git", "-C", a.Ctx.Workspace,
		"worktree", "add", wt, "-b", branch)
	if out, err := cmd.CombinedOutput(); err != nil {
		return "", fmt.Errorf("%s", strings.TrimSpace(string(out)))
	}
	a.Worktrees = append(a.Worktrees, [2]string{wt, branch})
	return wt, nil
}

// childCtx returns a tool context rooted at workspace, inheriting policy.
func (a *Agent) childCtx(workspace string) *ToolCtx {
	c := NewToolCtx(workspace)
	c.Escalations = append([]string{}, a.Ctx.Escalations...)
	c.NetAllow = a.Ctx.NetAllow
	c.NoSandbox = a.Ctx.NoSandbox
	c.LargeReadLines = a.Ctx.LargeReadLines
	c.MCPServers = a.Ctx.MCPServers
	return c
}

// cleanup removes clean worktrees (contract §10). Called via defer.
func (a *Agent) cleanup() {
	for _, w := range a.Worktrees {
		wt, branch := w[0], w[1]
		out, err := exec.Command("git", "-C", wt,
			"status", "--porcelain").Output()
		if err != nil || strings.TrimSpace(string(out)) != "" {
			continue
		}
		exec.Command("git", "-C", a.Ctx.Workspace,
			"worktree", "remove", wt).Run()
		exec.Command("git", "-C", a.Ctx.Workspace,
			"branch", "-D", branch).Run()
	}
	a.Worktrees = nil
}

func (a *Agent) subagent(prompt string, worktree bool, emit func(Event)) string {
	child := sessionCreate(a.Home, a.Session.Harness)
	wtNote := ""
	if worktree {
		if _, err := a.addWorktree(child.ID); err != nil {
			wtNote = fmt.Sprintf("\n[worktree unavailable: %s]", err)
		} else {
			wtNote = fmt.Sprintf("\n[worktree branch: cosmos-task-%s]", child.ID)
		}
	}
	var childCtx *ToolCtx
	if n := len(a.Worktrees); n > 0 {
		childCtx = a.childCtx(a.Worktrees[n-1][0])
	}
	var wtPath any
	if n := len(a.Worktrees); n > 0 {
		wtPath = a.Worktrees[n-1][0]
	}
	emit(a.Session.append("subagent_start", map[string]any{
		"child_session": child.ID, "worktree": wtPath, "prompt": trunc(prompt, 200)}))
	child.append("session_start", map[string]any{
		"model": a.ModelAlias, "mode": "agent", "sandbox": "appcontainer",
		"boot_tokens": a.BootTokens, "parent_session": a.Session.ID})
	msgs := []map[string]any{{
		"role": "user", "content": []any{map[string]any{"type": "text", "text": prompt}},
	}}
	result := "subagent turn cap reached"
	for i := 0; i < 8; i++ {
		var text strings.Builder
		var calls []map[string]any
		req := map[string]any{
			"system": a.System, "tools": a.Tools, "messages": msgs,
			"session_id": child.ID, "max_tokens": 2048,
		}
		if err := complete(a.Provider, req, func(c map[string]any) {
			if c["type"] == "text" {
				text.WriteString(strv(c, "text"))
			} else if c["type"] == "tool_use" {
				calls = append(calls, c)
			}
		}, a.Allowed); err != nil {
			result = "subagent provider error"
			break
		}
		var blocks []any
		if text.Len() > 0 {
			blocks = append(blocks, map[string]any{"type": "text", "text": text.String()})
		}
		for _, c := range calls {
			blocks = append(blocks, map[string]any{"type": "tool_use",
				"id": c["id"], "name": c["name"], "input": c["input"]})
		}
		msgs = append(msgs, map[string]any{"role": "assistant", "content": blocks})
		if len(calls) == 0 {
			result = trunc(text.String(), 4000)
			break
		}
		for _, c := range calls {
			name := strv(c, "name")
			var out string
			if PlanTools[name] {
				inp, _ := c["input"].(map[string]any)
				tctx := a.Ctx
				if childCtx != nil {
					tctx = childCtx
				}
				out = runTool(tctx, name, inp)
			} else {
				out = "subagent tool denied: " + name
			}
			msgs = append(msgs, map[string]any{"role": "user", "content": []any{
				map[string]any{"type": "tool_result", "tool_use_id": c["id"],
					"content": out, "is_error": false}}})
		}
	}
	emit(a.Session.append("subagent_end", map[string]string{"child_session": child.ID}))
	return result + wtNote
}

func trunc(s string, n int) string {
	if len(s) <= n {
		return s
	}
	return s[:n]
}

func (a *Agent) maybeCompact(emit func(Event)) {
	est := func(m map[string]any) uint64 {
		var t uint64
		for _, b := range anySlice(m["content"]) {
			bm, _ := b.(map[string]any)
			if s := strv(bm, "text"); s != "" {
				t += uint64(len(s) / 4)
			} else if s := strv(bm, "content"); s != "" {
				t += uint64(len(s) / 4)
			} else {
				t += 8
			}
		}
		return t
	}
	var total uint64 = uint64(a.BootTokens)
	for _, m := range a.Messages {
		total += est(m)
	}
	if total <= a.Provider.Context*8/10 {
		return
	}
	before := total
	head, tail := 2, 6
	if len(a.Messages) <= head+tail {
		return
	}
	dropped := len(a.Messages) - head - tail
	var next []map[string]any
	next = append(next, a.Messages[:head]...)
	next = append(next, map[string]any{"role": "user", "content": []any{
		map[string]any{"type": "text", "text": fmt.Sprintf(
			"[compaction: %d earlier messages elided — see session log for detail]", dropped)}}})
	next = append(next, a.Messages[len(a.Messages)-tail:]...)
	a.Messages = next
	runHooks(a.Cfg, "pre_compact", "", map[string]any{"before_tokens": before}, a.Session.ID)
	var after uint64 = uint64(a.BootTokens)
	for _, m := range a.Messages {
		after += est(m)
	}
	emit(a.Session.append("compaction", map[string]any{
		"before_tokens": before, "after_tokens": after, "reason": "context>80%"}))
}
