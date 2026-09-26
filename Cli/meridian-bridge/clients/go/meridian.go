// Package meridian is a thin Go client for `meridian serve --stdio` —
// newline-delimited JSON-RPC 2.0 to the meridian bridge child process.
//
// Binary resolution: MERIDIAN_BIN env, the workspace-relative build output
// (../target/{release,debug}/meridian.exe), then PATH.
package meridian

import (
	"bufio"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"sync"
)

// FindBinary resolves the meridian executable path.
func FindBinary() (string, error) {
	if v := os.Getenv("MERIDIAN_BIN"); v != "" {
		return v, nil
	}
	exe := "meridian"
	if runtime.GOOS == "windows" {
		exe = "meridian.exe"
	}
	// clients/go -> crate root is ../../..
	if self, err := os.Executable(); err == nil {
		_ = self
	}
	for _, profile := range []string{"release", "debug"} {
		cand := filepath.Join("..", "..", "target", profile, exe)
		if _, err := os.Stat(cand); err == nil {
			abs, _ := filepath.Abs(cand)
			return abs, nil
		}
	}
	if p, err := exec.LookPath("meridian"); err == nil {
		return p, nil
	}
	return "", fmt.Errorf("meridian binary not found; set MERIDIAN_BIN or build meridian-bridge")
}

type rpcRequest struct {
	JSONRPC string `json:"jsonrpc"`
	ID      int    `json:"id"`
	Method  string `json:"method"`
	Params  any    `json:"params"`
}

type rpcResponse struct {
	JSONRPC string          `json:"jsonrpc"`
	ID      int             `json:"id"`
	Result  json.RawMessage `json:"result"`
	Error   *struct {
		Code    int    `json:"code"`
		Message string `json:"message"`
	} `json:"error"`
}

// Bridge is one meridian child process. Calls are serialized.
type Bridge struct {
	cmd    *exec.Cmd
	stdin  *bufio.Writer
	stdout *bufio.Scanner
	mu     sync.Mutex
	nextID int
}

// Open spawns `meridian serve --stdio`. vault may be "" to use the default.
func Open(binary, vault string) (*Bridge, error) {
	if binary == "" {
		var err error
		binary, err = FindBinary()
		if err != nil {
			return nil, err
		}
	}
	cmd := exec.Command(binary, "serve", "--stdio")
	if vault != "" {
		cmd.Env = append(os.Environ(), "MERIDIAN_VAULT="+vault)
	}
	stdin, err := cmd.StdinPipe()
	if err != nil {
		return nil, err
	}
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		return nil, err
	}
	if err := cmd.Start(); err != nil {
		return nil, err
	}
	return &Bridge{
		cmd:    cmd,
		stdin:  bufio.NewWriter(stdin),
		stdout: bufio.NewScanner(stdout),
	}, nil
}

// Call issues one JSON-RPC request and decodes the result into out.
func (b *Bridge) Call(method string, params any, out any) error {
	b.mu.Lock()
	defer b.mu.Unlock()
	b.nextID++
	req := rpcRequest{JSONRPC: "2.0", ID: b.nextID, Method: method, Params: params}
	if err := json.NewEncoder(b.stdin).Encode(req); err != nil {
		return err
	}
	if err := b.stdin.Flush(); err != nil {
		return err
	}
	if !b.stdout.Scan() {
		return fmt.Errorf("meridian closed stdout")
	}
	var resp rpcResponse
	if err := json.Unmarshal(b.stdout.Bytes(), &resp); err != nil {
		return err
	}
	if resp.Error != nil {
		return fmt.Errorf("meridian: %s", resp.Error.Message)
	}
	if out == nil {
		return nil
	}
	return json.Unmarshal(resp.Result, out)
}

// BootContext is the `boot.context` result.
type BootContext struct {
	Text     string   `json:"text"`
	Tokens   int      `json:"tokens"`
	Warnings []string `json:"warnings"`
}

func (b *Bridge) BootContext(repo, harness string) (*BootContext, error) {
	var out BootContext
	err := b.Call("boot.context", map[string]any{"repo": repo, "harness": harness}, &out)
	return &out, err
}

// SearchHit is one `memory.search` result.
type SearchHit struct {
	ID    string  `json:"id"`
	Text  string  `json:"text"`
	Type  string  `json:"type"`
	Date  string  `json:"date"`
	Score float64 `json:"score"`
}

func (b *Bridge) MemorySearch(query string, k int, scope string, seenIDs []string) ([]SearchHit, error) {
	var out struct {
		Results []SearchHit `json:"results"`
	}
	err := b.Call("memory.search", map[string]any{
		"query": query, "k": k, "scope": scope, "seen_ids": seenIDs,
	}, &out)
	return out.Results, err
}

func (b *Bridge) MemoryWrite(mtype, text, scope, src string) (string, error) {
	var out struct {
		ID string `json:"id"`
	}
	err := b.Call("memory.write", map[string]any{
		"type": mtype, "text": text, "scope": scope, "src": src,
	}, &out)
	return out.ID, err
}

func (b *Bridge) CacheRead(path string, maxLines int) (string, error) {
	var out struct {
		Summary string `json:"summary"`
	}
	err := b.Call("cache.read", map[string]any{"path": path, "max_lines": maxLines}, &out)
	return out.Summary, err
}

func (b *Bridge) PlaybookGet(name string) (string, error) {
	var out struct {
		Body string `json:"body"`
	}
	err := b.Call("playbook.get", map[string]any{"name": name}, &out)
	return out.Body, err
}

func (b *Bridge) WorldUpsert(sessionID, markdown string) (string, error) {
	var out struct {
		Path string `json:"path"`
	}
	err := b.Call("world.upsert", map[string]any{"session_id": sessionID, "markdown": markdown}, &out)
	return out.Path, err
}

func (b *Bridge) WorldGet(sessionID string) (string, int, error) {
	var out struct {
		Markdown string `json:"markdown"`
		Tokens   int    `json:"tokens"`
	}
	err := b.Call("world.get", map[string]any{"session_id": sessionID}, &out)
	return out.Markdown, out.Tokens, err
}

func (b *Bridge) RefinePropose(target, diff, evidence string) (string, error) {
	var out struct {
		ID string `json:"id"`
	}
	err := b.Call("refine.propose", map[string]any{"target": target, "diff": diff, "evidence": evidence}, &out)
	return out.ID, err
}

func (b *Bridge) RefineApply(id string) error {
	return b.Call("refine.apply", map[string]any{"id": id}, nil)
}

func (b *Bridge) RefineRollback(id string) error {
	return b.Call("refine.rollback", map[string]any{"id": id}, nil)
}

func (b *Bridge) SessionSearch(query string, k int) ([]SearchHit, error) {
	var out struct {
		Results []SearchHit `json:"results"`
	}
	err := b.Call("session.search", map[string]any{"query": query, "k": k}, &out)
	return out.Results, err
}

func (b *Bridge) SessionRegister(sessionID, harness, logPath, title string) (string, error) {
	var out struct {
		Path string `json:"path"`
	}
	err := b.Call("session.register", map[string]any{
		"session_id": sessionID, "harness": harness, "log_path": logPath, "title": title,
	}, &out)
	return out.Path, err
}

// Close terminates the child.
func (b *Bridge) Close() {
	if b.cmd.Process != nil {
		_ = b.cmd.Process.Kill()
	}
	_ = b.cmd.Wait()
}
