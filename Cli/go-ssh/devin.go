// Devin subscription — models from the signed-in plan; Cosmos runs the tools.
// Auth: Devin CLI credentials.toml. Completions: Codeium GetChatMessage with
// our tool schemas. We never spawn `devin acp`.
package main

import (
	"bytes"
	"compress/gzip"
	"context"
	"encoding/binary"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
	"sync"
	"time"
)

const (
	devinDefaultServer = "https://server.codeium.com"
	devinSessionPrefix = "devin-session-token$"
	devinCLIVersion    = "3000.11.3"
	devinChatPath      = "/exa.api_server_pb.ApiServerService/GetChatMessage"
	devinModelsPath    = "/exa.api_server_pb.ApiServerService/GetCliModelConfigs"
	devinMaxFrame      = 16 << 20
)

type devinCache struct {
	at     time.Time
	ready  bool
	detail string
	models []CatalogModel
}

type devinCreds struct {
	key, server string
}

var (
	devinMu     sync.Mutex
	devinCached *devinCache
)

func devinExtraPaths() []string {
	home, _ := os.UserHomeDir()
	local := os.Getenv("LOCALAPPDATA")
	var out []string
	if home != "" {
		out = append(out, filepath.Join(home, ".local", "bin", "devin.exe"),
			filepath.Join(home, ".local", "bin", "devin.cmd"),
			filepath.Join(home, ".local", "bin", "devin"))
	}
	if local != "" {
		out = append(out,
			filepath.Join(local, "devin", "cli", "bin", "devin.exe"),
			filepath.Join(local, "devin", "cli", "bin", "devin.cmd"),
			filepath.Join(local, "Programs", "Devin", "bin", "devin.exe"),
			filepath.Join(local, "Programs", "Devin", "bin", "devin.cmd"),
			filepath.Join(local, "Programs", "Devin", "resources", "app", "extensions", "windsurf", "devin", "bin", "devin.exe"))
	}
	return out
}

func devinExecutable() string {
	if v := strings.TrimSpace(os.Getenv("DEVIN_EXECUTABLE")); v != "" {
		if st, err := os.Stat(v); err == nil && !st.IsDir() {
			return v
		}
	}
	names := []string{"devin.exe", "devin.cmd", "devin"}
	for _, dir := range strings.Split(os.Getenv("PATH"), string(os.PathListSeparator)) {
		for _, n := range names {
			p := filepath.Join(dir, n)
			if st, err := os.Stat(p); err == nil && !st.IsDir() {
				return p
			}
		}
	}
	for _, p := range devinExtraPaths() {
		if st, err := os.Stat(p); err == nil && !st.IsDir() {
			return p
		}
	}
	return ""
}

func hideWindow(c *exec.Cmd) {
	sysHideWindow(c)
}

func parseDevinList(raw []byte) ([]CatalogModel, error) {
	var catalog struct {
		Families []struct {
			Variants []struct {
				ModelUID string `json:"model_uid"`
				Label    string `json:"label"`
			} `json:"variants"`
		} `json:"families"`
	}
	if err := json.Unmarshal(raw, &catalog); err != nil {
		return nil, fmt.Errorf("invalid Devin model catalog: %w", err)
	}
	var models []CatalogModel
	seen := map[string]bool{}
	for _, f := range catalog.Families {
		for _, v := range f.Variants {
			id := strings.TrimSpace(v.ModelUID)
			if id == "" || seen[id] {
				continue
			}
			name := strings.TrimSpace(v.Label)
			if name == "" {
				name = id
			}
			seen[id] = true
			models = append(models, CatalogModel{ID: id, Name: name, Context: 200000, Output: 32000, Reasoning: true})
		}
	}
	if len(models) == 0 {
		return nil, fmt.Errorf("Devin returned an empty model list — is the CLI signed in?")
	}
	return models, nil
}

func parseDevinCLIModels(v map[string]any) []CatalogModel {
	var items []any
	if a, ok := v["clientModelConfigs"].([]any); ok {
		items = a
	} else if a, ok := v["client_model_configs"].([]any); ok {
		items = a
	}
	var models []CatalogModel
	seen := map[string]bool{}
	for _, it := range items {
		m, _ := it.(map[string]any)
		if m == nil {
			continue
		}
		id := strings.TrimSpace(strv(m, "modelUid"))
		if id == "" {
			id = strings.TrimSpace(strv(m, "model_uid"))
		}
		name := strings.TrimSpace(strv(m, "label"))
		if id == "" || seen[id] {
			continue
		}
		if name == "" {
			name = id
		}
		seen[id] = true
		models = append(models, CatalogModel{ID: id, Name: name, Context: 200000, Output: 32000, Reasoning: true})
	}
	return models
}

func devinCredentialsFile() string {
	if v := strings.TrimSpace(os.Getenv("DEVIN_CREDENTIALS")); v != "" {
		return v
	}
	home, _ := os.UserHomeDir()
	var cands []string
	if app := os.Getenv("APPDATA"); app != "" {
		cands = append(cands, filepath.Join(app, "devin", "credentials.toml"))
	}
	if home != "" {
		cands = append(cands, filepath.Join(home, "Library", "Application Support", "devin", "credentials.toml"))
		xdg := os.Getenv("XDG_DATA_HOME")
		if xdg == "" {
			xdg = filepath.Join(home, ".local", "share")
		}
		cands = append(cands, filepath.Join(xdg, "devin", "credentials.toml"))
	}
	for _, p := range cands {
		if st, err := os.Stat(p); err == nil && !st.IsDir() {
			return p
		}
	}
	if len(cands) > 0 {
		return cands[0]
	}
	return ""
}

func tomlBasicString(text, key string) string {
	for _, raw := range strings.Split(text, "\n") {
		line := strings.TrimSpace(raw)
		rest, ok := strings.CutPrefix(line, key)
		if !ok {
			continue
		}
		rest = strings.TrimSpace(rest)
		rest, ok = strings.CutPrefix(rest, "=")
		if !ok {
			continue
		}
		rest = strings.TrimSpace(rest)
		if !strings.HasPrefix(rest, "\"") {
			continue
		}
		var b strings.Builder
		esc := false
		for _, r := range rest[1:] {
			if esc {
				switch r {
				case 'n':
					b.WriteByte('\n')
				case 't':
					b.WriteByte('\t')
				case 'r':
					b.WriteByte('\r')
				default:
					b.WriteRune(r)
				}
				esc = false
				continue
			}
			if r == '\\' {
				esc = true
				continue
			}
			if r == '"' {
				return b.String()
			}
			b.WriteRune(r)
		}
	}
	return ""
}

func trustedDevinServer(url string) bool {
	h := strings.ToLower(catalogHostOf(url))
	return h == "server.codeium.com" || strings.HasSuffix(h, ".codeium.com") ||
		h == "api.devin.ai" || strings.HasSuffix(h, ".devin.ai") || strings.HasSuffix(h, ".windsurf.com")
}

func normalizeDevinKey(k string) string {
	if strings.HasPrefix(k, devinSessionPrefix) {
		return k
	}
	return devinSessionPrefix + k
}

func loadDevinCreds() *devinCreds {
	b, err := os.ReadFile(devinCredentialsFile())
	if err != nil {
		return nil
	}
	text := string(b)
	key := tomlBasicString(text, "windsurf_api_key")
	if key == "" {
		key = tomlBasicString(text, "devin_api_key")
	}
	if key == "" {
		return nil
	}
	server := tomlBasicString(text, "api_server_url")
	if server == "" {
		server = devinDefaultServer
	}
	server = strings.TrimRight(server, "/")
	if !trustedDevinServer(server) {
		return nil
	}
	return &devinCreds{key: normalizeDevinKey(key), server: server}
}

func devinSignedIn() bool { return loadDevinCreds() != nil }

func devinMetadata(key string) map[string]any {
	osName := runtime.GOOS
	if osName == "darwin" {
		osName = "macos"
	}
	return map[string]any{
		"apiKey": key, "ideName": "devin-cli", "ideVersion": devinCLIVersion,
		"extensionName": "chisel", "extensionVersion": devinCLIVersion,
		"locale": "en", "os": osName, "ideType": "chisel",
	}
}

func httpDevinModels(c *devinCreds) ([]CatalogModel, error) {
	body, _ := json.Marshal(map[string]any{"metadata": devinMetadata(c.key)})
	req, err := http.NewRequest("POST", c.server+devinModelsPath, bytes.NewReader(body))
	if err != nil {
		return nil, err
	}
	req.Header.Set("content-type", "application/json")
	req.Header.Set("connect-protocol-version", "1")
	resp, err := (&http.Client{Timeout: 20 * time.Second}).Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	raw, _ := io.ReadAll(io.LimitReader(resp.Body, 8<<20))
	if resp.StatusCode == 401 || resp.StatusCode == 403 {
		return nil, fmt.Errorf("Devin rejected the subscription (%d) — run `devin auth login`", resp.StatusCode)
	}
	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		return nil, fmt.Errorf("Devin model list failed (%d)", resp.StatusCode)
	}
	var v map[string]any
	if json.Unmarshal(raw, &v) != nil {
		return nil, fmt.Errorf("Devin model list wasn't JSON")
	}
	models := parseDevinCLIModels(v)
	return models, nil
}

func unionDevinModels(a, b []CatalogModel) []CatalogModel {
	seen := map[string]bool{}
	var out []CatalogModel
	for _, m := range append(append([]CatalogModel{}, a...), b...) {
		if m.ID == "" || seen[m.ID] {
			continue
		}
		seen[m.ID] = true
		if m.Name == "" {
			m.Name = m.ID
		}
		out = append(out, m)
	}
	return out
}

func cliDevinModels() ([]CatalogModel, error) {
	exe := devinExecutable()
	if exe == "" {
		return nil, fmt.Errorf("Devin CLI is not installed")
	}
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	c := exec.CommandContext(ctx, exe, "models", "list", "--format", "json")
	hideWindow(c)
	out, err := c.Output()
	if err != nil {
		msg := "not signed in"
		if ctx.Err() == context.DeadlineExceeded {
			msg = "Devin model list timed out"
		} else if ee, ok := err.(*exec.ExitError); ok {
			if s := strings.TrimSpace(string(ee.Stderr)); s != "" {
				if i := strings.IndexAny(s, "\r\n"); i > 0 {
					s = s[:i]
				}
				msg = s
				if len(msg) > 80 {
					msg = msg[:80]
				}
			}
		}
		return nil, fmt.Errorf("%s", msg)
	}
	return parseDevinList(out)
}

func truncErr(s string) string {
	s = strings.TrimSpace(s)
	if len(s) > 80 {
		return s[:80]
	}
	return s
}

func devinProbe() devinCache {
	errMsg := "run `devin auth login`"
	var models []CatalogModel
	if c := loadDevinCreds(); c != nil {
		if m, err := httpDevinModels(c); err == nil {
			models = m
		} else {
			errMsg = err.Error()
		}
	}
	if m, err := cliDevinModels(); err == nil {
		models = unionDevinModels(models, m)
	} else if len(models) == 0 {
		errMsg = err.Error()
	}
	if len(models) == 0 {
		return devinCache{at: time.Now(), detail: truncErr(errMsg)}
	}
	return devinCache{at: time.Now(), ready: true, detail: fmt.Sprintf("signed in · %d models", len(models)), models: models}
}

func devinStatus() devinCache {
	devinMu.Lock()
	defer devinMu.Unlock()
	if devinCached != nil && time.Since(devinCached.at) < 45*time.Second {
		return *devinCached
	}
	c := devinProbe()
	devinCached = &c
	return c
}

func devinInvalidate() {
	devinMu.Lock()
	devinCached = nil
	devinMu.Unlock()
}

func devinKnownReady() bool {
	if devinSignedIn() {
		return true
	}
	devinMu.Lock()
	c := devinCached
	devinMu.Unlock()
	if c != nil {
		return c.ready
	}
	return len(liveCached("devin")) > 0
}

func devinReady() bool {
	if devinSignedIn() {
		return true
	}
	return devinStatus().ready
}

func devinDetail() string {
	devinMu.Lock()
	c := devinCached
	devinMu.Unlock()
	if c != nil {
		return c.detail
	}
	if devinSignedIn() {
		return "signed in"
	}
	return "run `devin auth login`"
}

func devinModels() ([]CatalogModel, error) {
	devinInvalidate()
	c := devinStatus()
	if !c.ready {
		return nil, fmt.Errorf("%s", c.detail)
	}
	return c.models, nil
}

func devinHint() string {
	if devinSignedIn() {
		return "Devin is signed in. /providers → Devin to load the live model list."
	}
	if devinExecutable() == "" {
		return "Devin isn't signed in. Windows: irm https://static.devin.ai/cli/setup.ps1 | iex — then `devin auth login`."
	}
	return "Devin CLI found but not signed in (" + devinDetail() + ") — run `devin auth login`, then /providers again."
}

func encodeConnectFrame(payload []byte) []byte {
	out := make([]byte, 5+len(payload))
	out[0] = 0
	binary.BigEndian.PutUint32(out[1:5], uint32(len(payload)))
	copy(out[5:], payload)
	return out
}

func gunzipBytes(b []byte) ([]byte, error) {
	r, err := gzip.NewReader(bytes.NewReader(b))
	if err != nil {
		return nil, err
	}
	defer r.Close()
	return io.ReadAll(io.LimitReader(r, int64(devinMaxFrame)))
}

func decodeConnectJSON(buf []byte) ([]map[string]any, error) {
	var out []map[string]any
	i := 0
	for i+5 <= len(buf) {
		flags := buf[i]
		n := int(binary.BigEndian.Uint32(buf[i+1 : i+5]))
		if n > devinMaxFrame {
			return nil, fmt.Errorf("Devin Connect frame too large")
		}
		if i+5+n > len(buf) {
			return nil, fmt.Errorf("Devin Connect stream ended with a partial frame")
		}
		payload := buf[i+5 : i+5+n]
		i += 5 + n
		if flags&1 != 0 {
			raw, err := gunzipBytes(payload)
			if err != nil {
				return nil, fmt.Errorf("Devin gzip frame: %w", err)
			}
			payload = raw
		}
		if flags&2 != 0 {
			var v map[string]any
			if json.Unmarshal(payload, &v) == nil {
				if errv, ok := v["error"].(map[string]any); ok {
					code := strv(errv, "code")
					msg := strv(errv, "message")
					if code != "" || msg != "" {
						return nil, fmt.Errorf("Devin stream error%s: %s", ifs(code != "", " "+code, ""), msg)
					}
				}
			}
			continue
		}
		if len(payload) == 0 {
			continue
		}
		var v map[string]any
		if json.Unmarshal(payload, &v) == nil {
			out = append(out, v)
		}
	}
	return out, nil
}

func ifs(cond bool, a, b string) string {
	if cond {
		return a
	}
	return b
}

func chatPrompts(req map[string]any) []any {
	cascade := strv(req, "session_id")
	if cascade == "" {
		cascade = "cosmos"
	}
	var prompts []any
	idx := 0
	for _, m := range anySlice(req["messages"]) {
		mm, _ := m.(map[string]any)
		role := strv(mm, "role")
		content := anySlice(mm["content"])
		if role == "assistant" {
			var text strings.Builder
			var calls []any
			for _, part := range content {
				pm, _ := part.(map[string]any)
				switch strv(pm, "type") {
				case "text":
					if text.Len() > 0 {
						text.WriteByte('\n')
					}
					text.WriteString(strv(pm, "text"))
				case "tool_use":
					args, _ := json.Marshal(pm["input"])
					calls = append(calls, map[string]any{
						"id": strv(pm, "id"), "name": strv(pm, "name"),
						"argumentsJson": string(args),
					})
				}
			}
			idx++
			p := map[string]any{"messageId": fmt.Sprintf("%s-%d", cascade, idx), "source": 2, "prompt": text.String()}
			if len(calls) > 0 {
				p["toolCalls"] = calls
			}
			prompts = append(prompts, p)
			continue
		}
		for _, part := range content {
			pm, _ := part.(map[string]any)
			switch strv(pm, "type") {
			case "text":
				if t := strv(pm, "text"); t != "" {
					idx++
					prompts = append(prompts, map[string]any{
						"messageId": fmt.Sprintf("%s-%d", cascade, idx), "source": 1, "prompt": t,
					})
				}
			case "tool_result":
				idx++
				prompts = append(prompts, map[string]any{
					"messageId": fmt.Sprintf("%s-%d", cascade, idx), "source": 4,
					"prompt": strv(pm, "content"), "toolCallId": strv(pm, "tool_use_id"),
					"toolResultIsError": pm["is_error"] == true,
				})
			}
		}
	}
	return prompts
}

func numAny(v any) uint64 {
	switch t := v.(type) {
	case float64:
		if t > 0 {
			return uint64(t)
		}
	case json.Number:
		n, _ := t.Int64()
		if n > 0 {
			return uint64(n)
		}
	case string:
		n, _ := strconv.ParseUint(t, 10, 64)
		return n
	}
	return 0
}

func completeDevin(cfg ProviderCfg, req map[string]any, sink Sink) error {
	creds := loadDevinCreds()
	if creds == nil {
		return fmt.Errorf("%s", devinHint())
	}
	var tools []any
	for _, t := range anySlice(req["tools"]) {
		tm, _ := t.(map[string]any)
		schema, _ := json.Marshal(tm["input_schema"])
		if string(schema) == "null" {
			schema, _ = json.Marshal(tm["parameters"])
		}
		tools = append(tools, map[string]any{
			"name": strv(tm, "name"), "description": strv(tm, "description"),
			"jsonSchemaString": string(schema), "strict": false,
		})
	}
	sys := []string{}
	for _, s := range anySlice(req["system"]) {
		if t, ok := s.(string); ok {
			sys = append(sys, t)
		}
	}
	maxTok := "4096"
	if n := numAny(req["max_tokens"]); n > 0 {
		maxTok = strconv.FormatUint(n, 10)
	}
	cascade := strv(req, "session_id")
	if cascade == "" {
		cascade = newID()
	}
	body := map[string]any{
		"metadata":           devinMetadata(creds.key),
		"prompt":             strings.Join(sys, "\n\n"),
		"chatMessagePrompts": chatPrompts(req),
		"chatModelUid":       strings.TrimSpace(cfg.Model),
		"requestType":        5,
		"plannerMode":        1,
		"disableParallelToolCalls": true,
		"cascadeId":          cascade,
		"executionId":        newID(),
		"systemPromptCacheOptions": map[string]any{"type": 1},
		"configuration": map[string]any{
			"numCompletions": "1", "maxTokens": maxTok, "maxNewlines": "200",
			"temperature": 0.4, "firstTemperature": 0.4, "topK": "50", "topP": 1,
			"stopPatterns": []string{"<|user|>", "<|bot|>", "<|context_request|>", "<|endoftext|>", "<|end_of_turn|>"},
			"fimEotProbThreshold": 1,
		},
	}
	if len(tools) > 0 {
		body["tools"] = tools
		body["toolChoice"] = map[string]any{"optionName": "auto"}
	}
	prompts, _ := body["chatMessagePrompts"].([]any)
	if len(prompts) == 0 {
		return fmt.Errorf("nothing to send to Devin")
	}
	payload, err := json.Marshal(body)
	if err != nil {
		return err
	}
	httpReq, err := http.NewRequest("POST", creds.server+devinChatPath, bytes.NewReader(encodeConnectFrame(payload)))
	if err != nil {
		return err
	}
	httpReq.Header.Set("content-type", "application/connect+json")
	httpReq.Header.Set("connect-protocol-version", "1")
	resp, err := (&http.Client{Timeout: 180 * time.Second}).Do(httpReq)
	if err != nil {
		return fmt.Errorf("couldn't reach Devin: %w", err)
	}
	defer resp.Body.Close()
	raw, _ := io.ReadAll(io.LimitReader(resp.Body, int64(devinMaxFrame)))
	if resp.StatusCode == 401 || resp.StatusCode == 403 {
		return fmt.Errorf("Devin rejected the subscription (%d) — run `devin auth login`", resp.StatusCode)
	}
	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		preview := string(raw)
		if len(preview) > 200 {
			preview = preview[:200]
		}
		return fmt.Errorf("Devin chat failed (%d): %s", resp.StatusCode, preview)
	}
	frames, err := decodeConnectJSON(raw)
	if err != nil {
		return err
	}
	type acc struct{ id, name, args string }
	var toolsAcc []acc
	var usage Usage
	for _, v := range frames {
		if t := strv(v, "deltaText"); t == "" {
			t = strv(v, "delta_text")
			if t != "" {
				sink(map[string]any{"type": "text", "text": t})
			}
		} else {
			sink(map[string]any{"type": "text", "text": t})
		}
		calls := anySlice(v["deltaToolCalls"])
		if len(calls) == 0 {
			calls = anySlice(v["delta_tool_calls"])
		}
		for _, c := range calls {
			cm, _ := c.(map[string]any)
			id := strv(cm, "id")
			name := strv(cm, "name")
			args := strv(cm, "argumentsJson")
			if args == "" {
				args = strv(cm, "arguments_json")
			}
			if id != "" {
				found := false
				for i := range toolsAcc {
					if toolsAcc[i].id == id {
						if name != "" {
							toolsAcc[i].name = name
						}
						if strings.HasPrefix(args, toolsAcc[i].args) {
							toolsAcc[i].args = args
						} else {
							toolsAcc[i].args += args
						}
						found = true
						break
					}
				}
				if !found {
					toolsAcc = append(toolsAcc, acc{id: id, name: name, args: args})
				}
			} else if n := len(toolsAcc); n > 0 {
				if name != "" {
					toolsAcc[n-1].name = name
				}
				toolsAcc[n-1].args += args
			}
		}
		if u, ok := v["usage"].(map[string]any); ok {
			if n := numAny(u["inputTokens"]); n == 0 {
				n = numAny(u["input_tokens"])
				if n > 0 {
					usage.Input = n
				}
			} else {
				usage.Input = n
			}
			if n := numAny(u["outputTokens"]); n == 0 {
				n = numAny(u["output_tokens"])
				if n > 0 {
					usage.Output = n
				}
			} else {
				usage.Output = n
			}
			if n := numAny(u["cacheReadTokens"]); n == 0 {
				n = numAny(u["cache_read_tokens"])
				if n > 0 {
					usage.CacheRead = n
				}
			} else {
				usage.CacheRead = n
			}
			if n := numAny(u["cacheWriteTokens"]); n == 0 {
				n = numAny(u["cache_write_tokens"])
				if n > 0 {
					usage.CacheWrite = n
				}
			} else {
				usage.CacheWrite = n
			}
		}
	}
	for _, t := range toolsAcc {
		if t.name == "" {
			continue
		}
		var input any
		if t.args == "" {
			input = map[string]any{}
		} else if json.Unmarshal([]byte(t.args), &input) != nil {
			input = map[string]any{}
		}
		id := t.id
		if id == "" {
			id = newID()
		}
		sink(map[string]any{"type": "tool_use", "id": id, "name": t.name, "input": input})
	}
	if usage.Output == 0 && usage.Input == 0 {
		usage.Output = 1
	}
	sink(map[string]any{"type": "done", "usage": usage})
	return nil
}
