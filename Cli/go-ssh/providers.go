// zproviders — openai-compatible / anthropic / google / fixture (contract §12:
// every request passes the host allowlist first). Streaming via SSE.
package main

import (
	"bufio"
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"strings"
	"time"
)

type ProviderCfg struct {
	Kind, BaseURL, APIKeyEnv, Model, Provider string
	Context                                   uint64
	Prices                                    map[string]float64
}

type Usage struct {
	Input, CacheRead, CacheWrite, Output uint64
}

// Chunks delivered to sink: {"type":"text","text":…}, {"type":"tool_use",…},
// {"type":"done","usage":Usage}, {"type":"err","text":…}
type Sink func(map[string]any)

func hostAllowed(url string, allowed []string) error {
	u := url
	if i := strings.Index(u, "://"); i >= 0 {
		u = u[i+3:]
	}
	host := strings.ToLower(strings.FieldsFunc(u, func(r rune) bool {
		return r == '/' || r == ':' || r == '?'
	})[0])
	for _, a := range allowed {
		a = strings.ToLower(strings.TrimSuffix(a, "/"))
		if host == a || strings.HasSuffix(host, "."+a) {
			return nil
		}
	}
	return fmt.Errorf("privacy: outbound host %s not on the allowlist", host)
}

func apiKey(cfg ProviderCfg) string {
	if cfg.APIKeyEnv != "" {
		if v := strings.TrimSpace(os.Getenv(cfg.APIKeyEnv)); v != "" {
			return v
		}
	}
	if cfg.Provider != "" {
		if k := credStored(cfg.Provider); k != "" {
			return k
		}
	}
	return ""
}

var httpClient = &http.Client{Timeout: 300 * time.Second}

func postSSE(url string, headers map[string]string, body any, onData func(string) bool) error {
	raw, _ := json.Marshal(body)
	req, err := http.NewRequest("POST", url, bytes.NewReader(raw))
	if err != nil {
		return err
	}
	req.Header.Set("content-type", "application/json")
	for k, v := range headers {
		req.Header.Set(k, v)
	}
	resp, err := httpClient.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	if resp.StatusCode >= 400 {
		b, _ := io.ReadAll(io.LimitReader(resp.Body, 400))
		return fmt.Errorf("provider %d: %s", resp.StatusCode, string(b))
	}
	sc := bufio.NewScanner(resp.Body)
	sc.Buffer(make([]byte, 1<<20), 1<<20)
	for sc.Scan() {
		line := sc.Text()
		if d, ok := strings.CutPrefix(line, "data:"); ok {
			d = strings.TrimSpace(d)
			if d == "[DONE]" {
				break
			}
			if onData(d) {
				break
			}
		}
	}
	return sc.Err()
}

func openaiMessages(msgs []any) []map[string]any {
	var out []map[string]any
	for _, raw := range msgs {
		m, _ := raw.(map[string]any)
		role := strv(m, "role")
		switch role {
		case "user":
			for _, b := range anySlice(m["content"]) {
				bm, _ := b.(map[string]any)
				switch bm["type"] {
				case "text":
					out = append(out, map[string]any{"role": "user", "content": bm["text"]})
				case "tool_result":
					out = append(out, map[string]any{"role": "tool",
						"tool_call_id": bm["tool_use_id"], "content": bm["content"]})
				}
			}
		case "assistant":
			var text strings.Builder
			var calls []any
			for _, b := range anySlice(m["content"]) {
				bm, _ := b.(map[string]any)
				switch bm["type"] {
				case "text":
					if text.Len() > 0 {
						text.WriteString("\n")
					}
					fmt.Fprint(&text, bm["text"])
				case "tool_use":
					args, _ := json.Marshal(bm["input"])
					calls = append(calls, map[string]any{
						"id": bm["id"], "type": "function",
						"function": map[string]any{"name": bm["name"], "arguments": string(args)},
					})
				}
			}
			msg := map[string]any{"role": "assistant"}
			if text.Len() > 0 {
				msg["content"] = text.String()
			}
			if len(calls) > 0 {
				msg["tool_calls"] = calls
			}
			out = append(out, msg)
		}
	}
	return out
}

func completeOpenAI(cfg ProviderCfg, req map[string]any, sink Sink, allowed []string) error {
	url := strings.TrimSuffix(cfg.BaseURL, "/") + "/chat/completions"
	if err := hostAllowed(url, allowed); err != nil {
		return err
	}
	tools := anySlice(req["tools"])
	var toolDefs []any
	for _, t := range tools {
		tm, _ := t.(map[string]any)
		toolDefs = append(toolDefs, map[string]any{"type": "function", "function": map[string]any{
			"name": tm["name"], "description": tm["description"], "parameters": tm["input_schema"]}})
	}
	var sysMsgs []map[string]any
	for _, s := range anySlice(req["system"]) {
		sysMsgs = append(sysMsgs, map[string]any{"role": "system", "content": s})
	}
	msgs := append(sysMsgs, openaiMessages(anySlice(req["messages"]))...)
	body := map[string]any{
		"model": cfg.Model, "messages": msgs, "max_tokens": req["max_tokens"],
		"stream": true, "stream_options": map[string]any{"include_usage": true},
		"prompt_cache_key": req["session_id"],
	}
	if len(toolDefs) > 0 {
		body["tools"] = toolDefs
	}
	headers := map[string]string{}
	if k := apiKey(cfg); k != "" {
		headers["authorization"] = "Bearer " + k
	}
	acc := map[int]*struct{ id, name, args string }{}
	usageSeen := false
	err := postSSE(url, headers, body, func(data string) bool {
		var v map[string]any
		if json.Unmarshal([]byte(data), &v) != nil {
			return false
		}
		if u, ok := v["usage"].(map[string]any); ok {
			usageSeen = true
			cached := uint64(0)
			if d, ok := u["prompt_tokens_details"].(map[string]any); ok {
				cached = uint64(numv(d, "cached_tokens"))
			}
			sink(map[string]any{"type": "done", "usage": Usage{
				Input:     uint64(numv(u, "prompt_tokens")) - cached,
				CacheRead: cached, CacheWrite: 0,
				Output: uint64(numv(u, "completion_tokens")),
			}})
		}
		for _, ch := range anySlice(v["choices"]) {
			cm, _ := ch.(map[string]any)
			d, _ := cm["delta"].(map[string]any)
			if t, ok := d["content"].(string); ok && t != "" {
				sink(map[string]any{"type": "text", "text": t})
			}
			for _, tc := range anySlice(d["tool_calls"]) {
				tm, _ := tc.(map[string]any)
				idx := int(numv(tm, "index"))
				slot := acc[idx]
				if slot == nil {
					slot = &struct{ id, name, args string }{}
					acc[idx] = slot
				}
				if id := strv(tm, "id"); id != "" {
					slot.id = id
				}
				if fn, ok := tm["function"].(map[string]any); ok {
					slot.name += strv(fn, "name")
					slot.args += strv(fn, "arguments")
				}
			}
		}
		return false
	})
	if err != nil {
		return err
	}
	for _, slot := range acc {
		if slot.id != "" {
			var inp map[string]any
			json.Unmarshal([]byte(slot.args), &inp)
			sink(map[string]any{"type": "tool_use", "id": slot.id,
				"name": slot.name, "input": inp})
		}
	}
	if !usageSeen {
		sink(map[string]any{"type": "done", "usage": Usage{}})
	}
	return nil
}

func completeAnthropic(cfg ProviderCfg, req map[string]any, sink Sink, allowed []string) error {
	url := strings.TrimSuffix(cfg.BaseURL, "/") + "/v1/messages"
	if err := hostAllowed(url, allowed); err != nil {
		return err
	}
	var sys []map[string]any
	sysRaw := anySlice(req["system"])
	for i, s := range sysRaw {
		b := map[string]any{"type": "text", "text": s}
		if i == len(sysRaw)-1 {
			b["cache_control"] = map[string]any{"type": "ephemeral"}
		}
		sys = append(sys, b)
	}
	var toolDefs []map[string]any
	tools := anySlice(req["tools"])
	for i, t := range tools {
		tm, _ := t.(map[string]any)
		o := map[string]any{"name": tm["name"], "description": tm["description"],
			"input_schema": tm["input_schema"]}
		if i == len(tools)-1 {
			o["cache_control"] = map[string]any{"type": "ephemeral"}
		}
		toolDefs = append(toolDefs, o)
	}
	var msgs []map[string]any
	for _, m := range anySlice(req["messages"]) {
		mm, _ := m.(map[string]any)
		role := strv(mm, "role")
		var parts []any
		for _, b := range anySlice(mm["content"]) {
			bm, _ := b.(map[string]any)
			switch bm["type"] {
			case "text":
				parts = append(parts, map[string]any{"type": "text", "text": bm["text"]})
			case "tool_use":
				parts = append(parts, map[string]any{"type": "tool_use", "id": bm["id"],
					"name": bm["name"], "input": bm["input"]})
			case "tool_result":
				parts = append(parts, map[string]any{"type": "tool_result",
					"tool_use_id": bm["tool_use_id"],
					"content":     []any{map[string]any{"type": "text", "text": bm["content"]}},
					"is_error":    bm["is_error"]})
			}
		}
		if len(parts) > 0 {
			r := "user"
			if role == "assistant" {
				r = "assistant"
			}
			msgs = append(msgs, map[string]any{"role": r, "content": parts})
		}
	}
	body := map[string]any{
		"model": cfg.Model, "system": sys, "messages": msgs,
		"tools": toolDefs, "max_tokens": req["max_tokens"], "stream": true,
	}
	headers := map[string]string{"anthropic-version": "2023-06-01"}
	if k := apiKey(cfg); k != "" {
		headers["x-api-key"] = k
	}
	acc := map[int]*struct{ id, name, args string }{}
	err := postSSE(url, headers, body, func(data string) bool {
		var v map[string]any
		if json.Unmarshal([]byte(data), &v) != nil {
			return false
		}
		switch strv(v, "type") {
		case "content_block_start":
			if cb, ok := v["content_block"].(map[string]any); ok && cb["type"] == "tool_use" {
				i := int(numv(v, "index"))
				acc[i] = &struct{ id, name, args string }{
					id: strv(cb, "id"), name: strv(cb, "name")}
			}
		case "content_block_delta":
			i := int(numv(v, "index"))
			if d, ok := v["delta"].(map[string]any); ok {
				if d["type"] == "text_delta" {
					if t := strv(d, "text"); t != "" {
						sink(map[string]any{"type": "text", "text": t})
					}
				} else if d["type"] == "input_json_delta" {
					if slot := acc[i]; slot != nil {
						slot.args += strv(d, "partial_json")
					}
				}
			}
		case "message_start", "message_delta":
			u, _ := v["usage"].(map[string]any)
			if msg, ok := v["message"].(map[string]any); ok {
				if mu, ok := msg["usage"].(map[string]any); ok {
					for k, vv := range mu {
						if u == nil {
							u = map[string]any{}
						}
						if _, seen := u[k]; !seen {
							u[k] = vv
						}
					}
				}
			}
			sink(map[string]any{"type": "done", "usage": Usage{
				Input:      uint64(numv(u, "input_tokens")),
				CacheRead:  uint64(numv(u, "cache_read_input_tokens")),
				CacheWrite: uint64(numv(u, "cache_creation_input_tokens")),
				Output:     uint64(numv(u, "output_tokens")),
			}})
		}
		return false
	})
	if err != nil {
		return err
	}
	for _, slot := range acc {
		if slot.name != "" {
			var inp map[string]any
			json.Unmarshal([]byte(slot.args), &inp)
			sink(map[string]any{"type": "tool_use", "id": slot.id,
				"name": slot.name, "input": inp})
		}
	}
	return nil
}

func completeGoogle(cfg ProviderCfg, req map[string]any, sink Sink, allowed []string) error {
	url := strings.TrimSuffix(cfg.BaseURL, "/") +
		fmt.Sprintf("/v1beta/models/%s:streamGenerateContent?alt=sse", cfg.Model)
	if err := hostAllowed(url, allowed); err != nil {
		return err
	}
	var sys []any
	for _, s := range anySlice(req["system"]) {
		sys = append(sys, map[string]any{"text": s})
	}
	var contents []any
	for _, m := range anySlice(req["messages"]) {
		mm, _ := m.(map[string]any)
		role := "user"
		if strv(mm, "role") == "assistant" {
			role = "model"
		}
		var parts []any
		for _, b := range anySlice(mm["content"]) {
			bm, _ := b.(map[string]any)
			switch bm["type"] {
			case "text":
				parts = append(parts, map[string]any{"text": bm["text"]})
			case "tool_use":
				parts = append(parts, map[string]any{"functionCall": map[string]any{"name": bm["name"], "args": bm["input"]}})
			case "tool_result":
				parts = append(parts, map[string]any{"functionResponse": map[string]any{"name": "tool",
					"response": map[string]any{"output": bm["content"]}}})
			}
		}
		if len(parts) > 0 {
			contents = append(contents, map[string]any{"role": role, "parts": parts})
		}
	}
	var decls []any
	for _, t := range anySlice(req["tools"]) {
		tm, _ := t.(map[string]any)
		decls = append(decls, map[string]any{"name": tm["name"],
			"description": tm["description"], "parameters": tm["input_schema"]})
	}
	body := map[string]any{
		"systemInstruction": map[string]any{"parts": sys},
		"contents":          contents,
		"tools":             []any{map[string]any{"functionDeclarations": decls}},
	}
	headers := map[string]string{}
	if k := apiKey(cfg); k != "" {
		headers["x-goog-api-key"] = k
	}
	return postSSE(url, headers, body, func(data string) bool {
		var v map[string]any
		if json.Unmarshal([]byte(data), &v) != nil {
			return false
		}
		for _, cand := range anySlice(v["candidates"]) {
			cm, _ := cand.(map[string]any)
			content, _ := cm["content"].(map[string]any)
			for _, p := range anySlice(content["parts"]) {
				pm, _ := p.(map[string]any)
				if t := strv(pm, "text"); t != "" {
					sink(map[string]any{"type": "text", "text": t})
				}
				if fc, ok := pm["functionCall"].(map[string]any); ok {
					sink(map[string]any{"type": "tool_use", "id": newID(),
						"name": strv(fc, "name"), "input": fc["args"]})
				}
			}
		}
		if u, ok := v["usageMetadata"].(map[string]any); ok {
			cached := uint64(numv(u, "cachedContentTokenCount"))
			sink(map[string]any{"type": "done", "usage": Usage{
				Input:     uint64(numv(u, "promptTokenCount")) - cached,
				CacheRead: cached, CacheWrite: 0,
				Output: uint64(numv(u, "candidatesTokenCount")),
			}})
		}
		return false
	})
}

// fixture — deterministic offline provider. `ping`→`pong`; `tool:NAME {json}`→call.
func completeFixture(cfg ProviderCfg, req map[string]any, sink Sink) error {
	last := ""
	hasToolResult := false
	msgs := anySlice(req["messages"])
	if len(msgs) > 0 {
		mm, _ := msgs[len(msgs)-1].(map[string]any)
		for _, b := range anySlice(mm["content"]) {
			bm, _ := b.(map[string]any)
			if t := strv(bm, "text"); t != "" {
				last = t
			}
			if bm["type"] == "tool_result" {
				hasToolResult = true
			}
		}
	}
	est := func(s string) uint64 { return uint64(len(s) / 4) }
	var sysT uint64
	for _, s := range anySlice(req["system"]) {
		if st, ok := s.(string); ok {
			sysT += est(st)
		}
	}
	var msgT uint64
	for _, m := range msgs {
		mm, _ := m.(map[string]any)
		for _, b := range anySlice(mm["content"]) {
			bm, _ := b.(map[string]any)
			if t := strv(bm, "text"); t != "" {
				msgT += est(t)
			} else {
				msgT += 4
			}
		}
	}
	sink(map[string]any{"type": "done",
		"usage": Usage{Input: sysT + msgT}})
	if hasToolResult {
		sink(map[string]any{"type": "text", "text": "done."})
	} else if strings.HasPrefix(last, "tool:") {
		rest := last[5:]
		name := rest
		var inp map[string]any
		if i := strings.IndexByte(rest, ' '); i >= 0 {
			name = rest[:i]
			json.Unmarshal([]byte(rest[i+1:]), &inp)
		}
		sink(map[string]any{"type": "tool_use", "id": newID(), "name": name, "input": inp})
	} else if strings.Contains(strings.ToLower(last), "ping") {
		sink(map[string]any{"type": "text", "text": "pong"})
	} else {
		short := last
		if len(short) > 80 {
			short = short[:80]
		}
		sink(map[string]any{"type": "text", "text": fmt.Sprintf(
			"fixture: no live provider configured — received %q. Configure [providers] + [models] in config.toml for a real model.", short)})
	}
	sink(map[string]any{"type": "done", "usage": Usage{Output: 13}})
	return nil
}

func complete(cfg ProviderCfg, req map[string]any, sink Sink, allowed []string) error {
	switch cfg.Kind {
	case "fixture":
		return completeFixture(cfg, req, sink)
	case "openai":
		return completeOpenAI(cfg, req, sink, allowed)
	case "anthropic":
		return completeAnthropic(cfg, req, sink, allowed)
	case "google":
		return completeGoogle(cfg, req, sink, allowed)
	case "devin":
		return completeDevin(cfg, req, sink)
	}
	return fmt.Errorf("unknown provider kind: %s", cfg.Kind)
}
