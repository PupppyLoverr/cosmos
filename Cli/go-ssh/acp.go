// ACP mode (`zagy acp`): newline-delimited JSON-RPC over stdio (contract §11).
package main

import (
	"bufio"
	"encoding/json"
	"fmt"
	"os"
)

func acpSend(v map[string]any) {
	b, _ := json.Marshal(v)
	fmt.Println(string(b))
}

func acpResult(id any, r map[string]any) {
	acpSend(map[string]any{"jsonrpc": "2.0", "id": id, "result": r})
}

func acpErr(id any, code int, msg string) {
	acpSend(map[string]any{"jsonrpc": "2.0", "id": id,
		"error": map[string]any{"code": code, "message": msg}})
}

func acpUpdate(sid string, ev Event) map[string]any {
	var u map[string]any
	var d map[string]any
	_ = json.Unmarshal(ev.Data, &d)
	switch ev.Kind {
	case "assistant":
		u = map[string]any{"sessionUpdate": "agent_message_chunk",
			"content": map[string]any{"type": "text", "text": strv(d, "text")}}
	case "tool_call":
		u = map[string]any{"sessionUpdate": "tool_call",
			"toolCallId": strv(d, "id"), "title": strv(d, "name")}
	case "tool_result":
		status := "completed"
		if b, _ := d["is_error"].(bool); b {
			status = "failed"
		}
		u = map[string]any{"sessionUpdate": "tool_call_update",
			"toolCallId": strv(d, "tool_use_id"), "status": status,
			"content": []any{map[string]any{"type": "content",
				"content": map[string]any{"type": "text", "text": strv(d, "content")}}}}
	default:
		return nil
	}
	return map[string]any{"jsonrpc": "2.0", "method": "session/update",
		"params": map[string]any{"sessionId": sid, "update": u}}
}

func acpRun() int {
	sc := bufio.NewScanner(os.Stdin)
	sc.Buffer(make([]byte, 0, 1<<20), 1<<20)
	agents := map[string]*Agent{}
	for sc.Scan() {
		line := sc.Text()
		if line == "" {
			continue
		}
		var req map[string]any
		if json.Unmarshal([]byte(line), &req) != nil {
			continue
		}
		method := strv(req, "method")
		id := req["id"]
		params, _ := req["params"].(map[string]any)

		switch method {
		case "initialize":
			acpResult(id, map[string]any{
				"protocolVersion": 1,
				"agentInfo":       map[string]any{"name": "zagy", "version": "0.1.0"},
				"agentCapabilities": map[string]any{"loadSession": false,
					"promptCapabilities": map[string]any{"embeddedContext": true}},
			})
		case "session/new":
			cwd := strv(params, "cwd")
			if cwd == "" {
				cwd, _ = os.Getwd()
			}
			opts := &RunOpts{}
			a, err := NewAgent(opts, "zagy", cwd)
			if err != nil {
				acpErr(id, -32603, err.Error())
				continue
			}
			agents[a.Session.ID] = a
			acpResult(id, map[string]any{"sessionId": a.Session.ID})
		case "session/prompt":
			sid := strv(params, "sessionId")
			a := agents[sid]
			if a == nil {
				acpErr(id, -32602, "unknown sessionId")
				continue
			}
			var prompt string
			for _, b := range anySlice(params["prompt"]) {
				if m, ok := b.(map[string]any); ok {
					if prompt != "" {
						prompt += "\n"
					}
					prompt += strv(m, "text")
				}
			}
			emit := func(ev Event) {
				if n := acpUpdate(sid, ev); n != nil {
					acpSend(n)
				}
			}
			a.SendUser(prompt, emit)
			maxTurns := a.Opts.MaxTurns
			if maxTurns == 0 {
				maxTurns = 16
			}
			reason := "end_turn"
			for i := 0; i < maxTurns; i++ {
				done, err := a.Turn(emit)
				if err != nil {
					reason = "refusal"
					break
				}
				if done {
					break
				}
				var calls []map[string]any
				if len(a.Messages) > 0 {
					for _, b := range anySlice(a.Messages[len(a.Messages)-1]["content"]) {
						if m, ok := b.(map[string]any); ok && m["type"] == "tool_use" {
							calls = append(calls, m)
						}
					}
				}
				if len(calls) == 0 {
					break
				}
				for _, c := range calls {
					inp, _ := c["input"].(map[string]any)
					a.ExecTool(strv(c, "id"), strv(c, "name"), inp, emit)
				}
			}
			acpResult(id, map[string]any{"stopReason": reason})
		case "session/cancel":
			// notification — nothing to interrupt in a sync loop
		default:
			if id != nil {
				acpErr(id, -32601, "method not found")
			}
		}
	}
	return ExitDone
}
