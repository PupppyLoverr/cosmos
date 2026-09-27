// ztools — contract §2 tools, identical semantics to the Rust crate.
package main

import (
	"crypto/sha256"
	_ "embed"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"sort"
	"strconv"
	"strings"
	"time"
)

//go:embed tools.json
var ToolsJSON string

const ResultCap = 30_000
const LargeReadLines = 200

var PlanTools = map[string]bool{
	"read": true, "grep": true, "glob": true, "ls": true,
	"memory_search": true, "submit_plan": true, "tool_search": true,
	"context_remaining": true,
}

var ReadOnlyTools = map[string]bool{
	"read": true, "grep": true, "glob": true, "ls": true,
	"memory_search": true, "tool_search": true, "context_remaining": true,
}

type ToolCtx struct {
	Workspace       string
	AllowedRoots    []string
	ReadState       map[string]string
	Escalations     []string
	NetAllow        bool
	NoSandbox       bool
	SandboxDegraded string
	LargeReadLines  int
	MCPServers      map[string]any
	SessionID       string
	Home            string
	SandboxMode     string
	CurrentToolID   string
}

func NewToolCtx(workspace string) *ToolCtx {
	abs, err := filepath.Abs(workspace)
	if err != nil {
		abs = workspace
	}
	return &ToolCtx{
		Workspace:      filepath.Clean(abs),
		ReadState:      map[string]string{},
		LargeReadLines: LargeReadLines,
		MCPServers:     map[string]any{},
	}
}

func capResult(s string) string {
	if len(s) <= ResultCap {
		return s
	}
	return fmt.Sprintf("%s\n[truncated: %d chars > %d cap]", s[:ResultCap], len(s), ResultCap)
}

func resolve(ctx *ToolCtx, path string) (string, error) {
	var joined string
	if filepath.IsAbs(path) {
		joined = path
	} else {
		joined = filepath.Join(ctx.Workspace, path)
	}
	norm := filepath.Clean(joined)
	root := filepath.Clean(ctx.Workspace)
	allowed := norm == root || strings.HasPrefix(norm, root+string(os.PathSeparator))
	for _, extra := range ctx.AllowedRoots {
		extra = filepath.Clean(extra)
		if norm == extra || strings.HasPrefix(norm, extra+string(os.PathSeparator)) {
			allowed = true
		}
	}
	if !allowed {
		return "", fmt.Errorf("path escapes workspace: %s", path)
	}
	return norm, nil
}

func sha6(line string) string {
	h := sha256.Sum256([]byte(strings.TrimRight(line, "\r\n")))
	return hex.EncodeToString(h[:])[:6]
}

func fileSHA(data []byte) string {
	h := sha256.Sum256(data)
	return hex.EncodeToString(h[:])
}

func toolRead(ctx *ToolCtx, inp map[string]any) (string, error) {
	path, err := resolve(ctx, strv(inp, "path"))
	if err != nil {
		return "", err
	}
	data, err := os.ReadFile(path)
	if err != nil {
		return "", fmt.Errorf("read %s: %w", path, err)
	}
	hash := fileSHA(data)
	text := string(data)
	lines := strings.Split(text, "\n")
	n := len(lines)
	if n > 0 && lines[n-1] == "" && strings.HasSuffix(text, "\n") {
		n--
	}
	start := int(numv(inp, "start"))
	if start < 1 {
		start = 1
	}
	end := n
	if v := numv(inp, "end"); v > 0 && int(v) < n {
		end = int(v)
	}
	_, hasStart := inp["start"]
	_, hasEnd := inp["end"]
	if !hasStart && !hasEnd && n > ctx.LargeReadLines {
		return "", fmt.Errorf("large_read_guard: %d lines > %d limit — read with start/end ranges (<=200 lines) or fetch the meridian cache.read summary", n, ctx.LargeReadLines)
	}
	var b strings.Builder
	fmt.Fprintf(&b, "file_sha256=%s lines=%d\n", hash, n)
	for i := start; i <= end && i <= n; i++ {
		line := lines[i-1]
		fmt.Fprintf(&b, "%d:%s|%s\n", i, sha6(line), line)
	}
	ctx.ReadState[path] = hash
	return strings.TrimRight(b.String(), "\n"), nil
}

func parseAnchor(s string) (int, string, bool) {
	n, h, ok := strings.Cut(s, ":")
	if !ok {
		return 0, "", false
	}
	num, err := strconv.Atoi(n)
	return num, h, err == nil
}

func freshAnchors(lines []string, around, n int) string {
	lo, hi := around-5, around+5
	if lo < 1 {
		lo = 1
	}
	if hi > n {
		hi = n
	}
	var b strings.Builder
	for i := lo; i <= hi && i <= len(lines); i++ {
		fmt.Fprintf(&b, "%d:%s|%s\n", i, sha6(lines[i-1]), lines[i-1])
	}
	return b.String()
}

func denyWritePath(ctx *ToolCtx, p string) bool {
	if ctx.SandboxMode == "full" {
		return false
	}
	s := strings.ToLower(strings.ReplaceAll(p, "\\", "/"))
	return strings.Contains(s, "/.ssh/") || strings.HasSuffix(s, "/.ssh") ||
		strings.Contains(s, "/.gnupg/") || strings.Contains(s, "/.env/") ||
		strings.HasSuffix(s, "/.env") || strings.Contains(s, ".env.")
}

func denyEnvCommand(command string) bool {
	c := strings.ToLower(strings.ReplaceAll(command, "\\", "/"))
	return strings.Contains(c, "/.ssh") || strings.Contains(c, "/.gnupg") || strings.Contains(c, ".env")
}

func assertWritable(ctx *ToolCtx, p, verb string) error {
	if denyWritePath(ctx, p) {
		return fmt.Errorf("sandbox deny: write to %s blocked", p)
	}
	if ctx.SandboxMode == "read-only" {
		return fmt.Errorf("sandbox read-only: %s denied", verb)
	}
	return nil
}

func toolEdit(ctx *ToolCtx, inp map[string]any) (string, error) {
	path, err := resolve(ctx, strv(inp, "path"))
	if err != nil {
		return "", err
	}
	if err := assertWritable(ctx, path, "edit"); err != nil {
		return "", err
	}
	data, err := os.ReadFile(path)
	if err != nil {
		return "", fmt.Errorf("read %s: %w", path, err)
	}
	hash := fileSHA(data)
	if want := strv(inp, "file_sha256"); want != "" && want != hash {
		return "", fmt.Errorf("STALE: file_sha256 mismatch (want %s, have %s)", want, hash)
	}
	text := string(data)
	newline := "\n"
	if strings.Contains(text, "\r\n") {
		newline = "\r\n"
	}
	lines := strings.Split(text, "\n")
	n := len(lines)
	type pend struct {
		s, e    int
		newText string
	}
	var pending []pend
	var stale strings.Builder
	for _, e := range anySlice(inp["edits"]) {
		em, _ := e.(map[string]any)
		sN, sH, ok1 := parseAnchor(strv(em, "start"))
		eN, eH, ok2 := parseAnchor(strv(em, "end"))
		ok := ok1 && ok2 && sN >= 1 && eN >= sN && eN <= n &&
			sha6(lines[sN-1]) == sH && sha6(lines[eN-1]) == eH
		if !ok {
			fmt.Fprintf(&stale, "anchor mismatch near line %d..%d:\n%s\n", sN, eN,
				freshAnchors(lines, sN, n))
		} else {
			pending = append(pending, pend{sN, eN, strv(em, "new_text")})
		}
	}
	if stale.Len() > 0 {
		return "", fmt.Errorf("STALE\n%s", stale.String())
	}
	ckptSave(ctx.Home, ctx.SessionID, path)
	owned := append([]string{}, lines...)
	sort.Slice(pending, func(i, j int) bool { return pending[i].s > pending[j].s })
	for _, p := range pending {
		repl := strings.Split(p.newText, "\n")
		if len(repl) > 0 && repl[len(repl)-1] == "" && p.newText != "" {
			repl = repl[:len(repl)-1]
		}
		var next []string
		next = append(next, owned[:p.s-1]...)
		next = append(next, repl...)
		next = append(next, owned[p.e:]...)
		owned = next
	}
	body := strings.Join(owned, newline)
	if strings.HasSuffix(text, newline) && !strings.HasSuffix(body, newline) {
		body += newline
	}
	if err := atomicWrite(path, []byte(body)); err != nil {
		return "", err
	}
	return "ok " + path, nil
}

func toolWrite(ctx *ToolCtx, inp map[string]any) (string, error) {
	path, err := resolve(ctx, strv(inp, "path"))
	if err != nil {
		return "", err
	}
	if err := assertWritable(ctx, path, "write"); err != nil {
		return "", err
	}
	content := strv(inp, "content")
	if _, err := os.Stat(path); err == nil {
		data, _ := os.ReadFile(path)
		if ctx.ReadState[path] != fileSHA(data) {
			return "", fmt.Errorf("STALE: %s exists and was not read this session (or changed)", path)
		}
	}
	if _, err := os.Stat(path); err == nil {
		ckptSave(ctx.Home, ctx.SessionID, path)
	}
	os.MkdirAll(filepath.Dir(path), 0o755)
	if err := atomicWrite(path, []byte(content)); err != nil {
		return "", err
	}
	data, _ := os.ReadFile(path)
	ctx.ReadState[path] = fileSHA(data)
	return "ok " + path, nil
}

func atomicWrite(path string, data []byte) error {
	tmp := fmt.Sprintf("%s.ztmp%d", path, os.Getpid())
	if err := os.WriteFile(tmp, data, 0o644); err != nil {
		return err
	}
	if err := os.Rename(tmp, path); err != nil {
		werr := os.WriteFile(path, data, 0o644)
		os.Remove(tmp)
		return werr
	}
	return nil
}

func toolLs(ctx *ToolCtx, inp map[string]any) (string, error) {
	path, err := resolve(ctx, strv(inp, "path"))
	if err != nil {
		return "", err
	}
	if path == "" {
		path = ctx.Workspace
	}
	entries, err := os.ReadDir(path)
	if err != nil {
		return "", fmt.Errorf("ls %s: %w", path, err)
	}
	var out []string
	for _, e := range entries {
		n := e.Name()
		if e.IsDir() {
			n += "/"
		}
		out = append(out, n)
	}
	sort.Strings(out)
	return strings.Join(out, "\n"), nil
}

func gitignorePatterns(root string) []string {
	var pats []string
	data, err := os.ReadFile(filepath.Join(root, ".gitignore"))
	if err != nil {
		return pats
	}
	for _, line := range strings.Split(string(data), "\n") {
		line = strings.TrimSpace(line)
		if line != "" && !strings.HasPrefix(line, "#") {
			pats = append(pats, strings.TrimPrefix(line, "/"))
		}
	}
	return pats
}

func ignored(rel string, pats []string) bool {
	rel = filepath.ToSlash(rel)
	for _, p := range pats {
		if strings.HasSuffix(p, "/") {
			base := strings.TrimSuffix(p, "/")
			if rel == base || strings.HasPrefix(rel, base+"/") ||
				strings.Contains(rel, "/"+base+"/") {
				return true
			}
			continue
		}
		if ok, _ := filepath.Match(p, rel); ok {
			return true
		}
		if ok, _ := filepath.Match(p, filepath.Base(rel)); ok {
			return true
		}
	}
	return false
}

func walkFiles(ctx *ToolCtx, root string, yield func(full, rel string) bool) {
	pats := gitignorePatterns(ctx.Workspace)
	filepath.Walk(root, func(path string, info os.FileInfo, err error) error {
		if err != nil {
			return nil
		}
		rel, _ := filepath.Rel(ctx.Workspace, path)
		if info.IsDir() {
			if ignored(rel, pats) {
				return filepath.SkipDir
			}
			return nil
		}
		if ignored(rel, pats) {
			return nil
		}
		if !yield(path, rel) {
			return fmt.Errorf("stop")
		}
		return nil
	})
}

func toolGlob(ctx *ToolCtx, inp map[string]any) (string, error) {
	pattern := strv(inp, "pattern")
	if pattern == "" {
		pattern = "*"
	}
	var out []string
	walkFiles(ctx, ctx.Workspace, func(full, rel string) bool {
		ok, _ := filepath.Match(pattern, filepath.ToSlash(rel))
		if !ok {
			// support **/x patterns: match basename
			if strings.HasPrefix(pattern, "**/") {
				ok, _ = filepath.Match(pattern[3:], filepath.Base(rel))
			}
		}
		if ok {
			out = append(out, filepath.ToSlash(rel))
		}
		return len(out) < 500
	})
	sort.Strings(out)
	return strings.Join(out, "\n"), nil
}

func toolGrep(ctx *ToolCtx, inp map[string]any) (string, error) {
	re, err := regexp.Compile(strv(inp, "pattern"))
	if err != nil {
		return "", fmt.Errorf("bad regex: %v", err)
	}
	root := ctx.Workspace
	if p := strv(inp, "path"); p != "" {
		root, err = resolve(ctx, p)
		if err != nil {
			return "", err
		}
	}
	globPat := strv(inp, "glob")
	var out []string
	capped := false
	walkFiles(ctx, root, func(full, rel string) bool {
		if globPat != "" {
			ok, _ := filepath.Match(globPat, filepath.ToSlash(rel))
			if !ok {
				ok, _ = filepath.Match(globPat, filepath.Base(rel))
			}
			if !ok {
				return true
			}
		}
		data, err := os.ReadFile(full)
		if err != nil {
			return true
		}
		for i, line := range strings.Split(string(data), "\n") {
			if re.MatchString(line) {
				out = append(out, fmt.Sprintf("%s:%d:%s", filepath.ToSlash(rel), i+1, line))
				if len(out) >= 200 {
					capped = true
					return false
				}
			}
		}
		return true
	})
	s := strings.Join(out, "\n")
	if capped {
		s += "\n[cap: 200 matches]"
	}
	return s, nil
}

func findBin(names []string, env string) string {
	if v := os.Getenv(env); v != "" {
		if _, err := os.Stat(v); err == nil {
			return v
		}
	}
	for _, n := range names {
		if p, err := exec.LookPath(n); err == nil {
			return p
		}
	}
	if exe, err := os.Executable(); err == nil {
		for _, n := range names {
			c := filepath.Join(filepath.Dir(exe), n)
			if _, err := os.Stat(c); err == nil {
				return c
			}
		}
	}
	return ""
}

func shellArgv(command string) []string {
	for _, d := range filepath.SplitList(os.Getenv("PATH")) {
		b := filepath.Join(d, "bash.exe")
		if _, err := os.Stat(b); err == nil {
			return []string{b, "-lc", command}
		}
	}
	return []string{"powershell.exe", "-NoProfile", "-NonInteractive", "-Command", command}
}

func fenceShell(raw string, exit int, logPath string, omitted int) string {
	lines := strings.Split(raw, "\n")
	body := raw
	if len(lines) > 48 {
		om := omitted
		if om == 0 {
			om = len(raw) - len(strings.Join(lines[:24], "\n")) - len(strings.Join(lines[len(lines)-24:], "\n"))
		}
		body = strings.Join(lines[:24], "\n") + fmt.Sprintf("\n… %d chars omitted …\n", om) + strings.Join(lines[len(lines)-24:], "\n")
	}
	note := ""
	if exit == 1 && strings.TrimSpace(raw) == "" {
		note = "no matches"
	}
	return fmt.Sprintf("exit_code=%d log_path=%s omitted_chars=%d %s\n<<<UNTRUSTED_SHELL\n%s\n>>>",
		exit, filepath.ToSlash(logPath), omitted, note, body)
}

func toolBash(ctx *ToolCtx, inp map[string]any) (string, error) {
	command := strv(inp, "command")
	if denyEnvCommand(command) && ctx.SandboxMode != "full" {
		return "", fmt.Errorf("sandbox deny: command touches ~/.ssh, ~/.gnupg, or .env")
	}
	if ctx.SandboxMode == "read-only" {
		return "", fmt.Errorf("sandbox read-only: bash writes denied")
	}
	wait := strv(inp, "wait")
	if wait == "" {
		wait = "exit"
	}
	timeout := int(numv(inp, "timeout_s"))
	if timeout <= 0 {
		timeout = 30
	}
	if timeout > 600 {
		timeout = 600
	}
	home := ctx.Home
	if home == "" {
		home = cliHome()
	}
	sid := ctx.SessionID
	if sid == "" {
		sid = "adhoc"
	}
	logDir := filepath.Join(home, "logs", sid)
	logPath := filepath.Join(logDir, ctx.CurrentToolID+".log")
	if ctx.CurrentToolID == "" {
		logPath = filepath.Join(logDir, "bash.log")
	}
	if inp["from_offset"] != nil {
		if data, err := os.ReadFile(logPath); err == nil {
			off := int(numv(inp, "from_offset"))
			if off < 0 {
				off = 0
			}
			if off > len(data) {
				off = len(data)
			}
			return fenceShell(string(data[off:]), 0, logPath, 0), nil
		}
	}
	if wait == "now" || wait == "next_write" {
		os.MkdirAll(logDir, 0o755)
		argv := shellArgv(command)
		c := exec.Command(argv[0], argv[1:]...)
		c.Dir = ctx.Workspace
		f, _ := os.OpenFile(logPath, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0o644)
		if f != nil {
			c.Stdout = f
			c.Stderr = f
		}
		if err := c.Start(); err != nil {
			return "", err
		}
		pid := 0
		if c.Process != nil {
			pid = c.Process.Pid
		}
		go func() {
			c.Wait()
			if f != nil {
				f.Close()
			}
		}()
		if wait == "next_write" {
			deadline := time.Now().Add(5 * time.Second)
			for time.Now().Before(deadline) {
				if st, err := os.Stat(logPath); err == nil && st.Size() > 0 {
					break
				}
				time.Sleep(50 * time.Millisecond)
			}
		}
		off := 0
		if st, err := os.Stat(logPath); err == nil {
			off = int(st.Size())
		}
		b, _ := json.Marshal(map[string]any{"task_id": fmt.Sprintf("job-%d", pid), "pid": pid, "log_path": filepath.ToSlash(logPath), "offset": off})
		return string(b), nil
	}
	argv := shellArgv(command)
	if ctx.NoSandbox {
		c := exec.Command(argv[0], argv[1:]...)
		c.Dir = ctx.Workspace
		out, err := c.CombinedOutput()
		os.MkdirAll(logDir, 0o755)
		os.WriteFile(logPath, out, 0o644)
		code := 0
		if err != nil {
			code = 1
		}
		om := len(out) - 8000
		if om < 0 {
			om = 0
		}
		return fenceShell(string(out), code, logPath, om), nil
	}
	zsb := findBin([]string{"zsandbox.exe", "zsandbox"}, "ZSANDBOX_BIN")
	if zsb == "" {
		return "", fmt.Errorf("zsandbox.exe not found (set ZSANDBOX_BIN)")
	}
	note := ""
	if ctx.SandboxDegraded != "" {
		note = fmt.Sprintf("[sandbox:degraded: %s]\n", ctx.SandboxDegraded)
	}
	net := "deny"
	if ctx.NetAllow {
		net = "allow"
	}
	args := append([]string{"exec", "--workspace", ctx.Workspace, "--net", net,
		"--timeout", fmt.Sprint(timeout), "--"}, argv...)
	c := exec.Command(zsb, args...)
	c.Dir = ctx.Workspace
	type res struct {
		out []byte
		err error
	}
	ch := make(chan res, 1)
	go func() {
		out, err := c.CombinedOutput()
		ch <- res{out, err}
	}()
	select {
	case r := <-ch:
		s := note + string(r.out)
		code := 0
		if ee, ok := r.err.(*exec.ExitError); ok {
			code = ee.ExitCode()
			if code == 124 {
				s += "\n[timeout]"
			}
		}
		os.MkdirAll(logDir, 0o755)
		os.WriteFile(logPath, []byte(s), 0o644)
		om := len(s) - 8000
		if om < 0 {
			om = 0
		}
		return fenceShell(s, code, logPath, om), nil
	case <-time.After(time.Duration(timeout+30) * time.Second):
		c.Process.Kill()
		return note + fenceShell("[timeout]", 124, logPath, 0), nil
	}
}

func meridianRun(ctx *ToolCtx, args ...string) (string, error) {
	mer := findBin([]string{"meridian.exe", "meridian"}, "MERIDIAN_BIN")
	if mer == "" {
		return "", fmt.Errorf("meridian bridge binary not found (set MERIDIAN_BIN)")
	}
	c := exec.Command(mer, args...)
	out, err := c.CombinedOutput()
	if err != nil {
		return "", fmt.Errorf("meridian %s: %s", args[0], strings.TrimSpace(string(out)))
	}
	return strings.TrimSpace(string(out)), nil
}

func toolMemorySearch(ctx *ToolCtx, inp map[string]any) (string, error) {
	k := int(numv(inp, "k"))
	if k <= 0 {
		k = 5
	}
	args := []string{"search", "--query", strv(inp, "query"), "--k", fmt.Sprint(k)}
	if s := strv(inp, "scope"); s != "" {
		args = append(args, "--scope", s)
	}
	return meridianRun(ctx, args...)
}

func toolMemoryWrite(ctx *ToolCtx, inp map[string]any) (string, error) {
	ty := strv(inp, "type")
	if ty == "" {
		return "", fmt.Errorf("missing required field: type (decision|preference|map|correction|playbook)")
	}
	scope := strv(inp, "scope")
	if scope == "" {
		scope = "global"
	}
	return meridianRun(ctx, "write", "--type", ty, "--scope", scope,
		"--text", strv(inp, "text"))
}

func runTool(ctx *ToolCtx, name string, inp map[string]any) string {
	var out string
	var err error
	switch name {
	case "read":
		out, err = toolRead(ctx, inp)
	case "edit":
		out, err = toolEdit(ctx, inp)
	case "write":
		out, err = toolWrite(ctx, inp)
	case "ls":
		out, err = toolLs(ctx, inp)
	case "glob":
		out, err = toolGlob(ctx, inp)
	case "grep":
		out, err = toolGrep(ctx, inp)
	case "bash":
		out, err = toolBash(ctx, inp)
	case "memory_search":
		out, err = toolMemorySearch(ctx, inp)
	case "memory_write":
		out, err = toolMemoryWrite(ctx, inp)
	case "tool_search", "mcp_call":
		if len(ctx.MCPServers) == 0 {
			err = fmt.Errorf("no MCP servers configured")
		} else {
			err = fmt.Errorf("mcp client not configured for this harness run")
		}
	case "submit_plan", "task", "request_escalation":
		err = fmt.Errorf("%s is handled by the harness, not the tool layer", name)
	default:
		err = fmt.Errorf("unknown tool: %s", name)
	}
	if err != nil {
		return "ERROR: " + err.Error()
	}
	return capResult(out)
}
