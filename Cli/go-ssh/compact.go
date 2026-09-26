// Five-layer compaction. Same order as zcore/src/compact.rs and zdev/compact.ts.
package main

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"
)

const (
	snipAfter          = 48
	snipKeepHead       = 3
	snipKeepNewest     = 40
	microKeep          = 3
	thrashMinReduction = 0.20
)

type compactEvent struct {
	Kind string
	Data map[string]any
}

type compactOutcome struct {
	Events []compactEvent
	World  string
	Before int
	After  int
}

func prefixHash(system []string, toolsJSON string) string {
	h := sha256.New()
	for _, s := range system {
		h.Write([]byte(s))
		h.Write([]byte{0})
	}
	h.Write([]byte(toolsJSON))
	return hex.EncodeToString(h.Sum(nil))
}

func estimateTokens(messages []map[string]any, boot int) int {
	n := boot
	for _, m := range messages {
		for _, b := range anySlice(m["content"]) {
			bm, _ := b.(map[string]any)
			n += len(strv(bm, "text")+strv(bm, "content")) / 4
		}
	}
	return n
}

func cacheWarm(last *time.Time, prefixNow, prefixThen string, now time.Time, warm time.Duration) bool {
	return last != nil && now.Sub(*last) < warm && prefixNow == prefixThen
}

func persistTool(home, content string) (path, sha string, bytes, lines int) {
	sum := sha256.Sum256([]byte(content))
	sha = hex.EncodeToString(sum[:])
	dir := filepath.Join(home, "cache", "tool")
	os.MkdirAll(dir, 0o755)
	path = filepath.Join(dir, sha+".txt")
	if _, err := os.Stat(path); err != nil {
		os.WriteFile(path, []byte(content), 0o644)
	}
	return path, sha, len(content), strings.Count(content, "\n") + 1
}

func middleOut(content string) string {
	lines := strings.Split(content, "\n")
	if len(lines) <= 48 {
		return content
	}
	head := strings.Join(lines[:24], "\n")
	tail := strings.Join(lines[len(lines)-24:], "\n")
	omitted := len(content) - len(head) - len(tail)
	return fmt.Sprintf("%s\n… %d chars omitted …\n%s", head, omitted, tail)
}

func layer1(messages []map[string]any, home string, largeReadLines int) {
	capN := largeReadLines * 40
	if capN < 8000 {
		capN = 8000
	}
	for _, m := range messages {
		blocks := anySlice(m["content"])
		for i, b := range blocks {
			bm, _ := b.(map[string]any)
			if strv(bm, "type") != "tool_result" {
				continue
			}
			c := strv(bm, "content")
			if len(c) > capN || strings.Count(c, "\n") > largeReadLines {
				p, sha, bytes, lines := persistTool(home, c)
				bm["content"] = fmt.Sprintf("[tool_result persisted path=%s sha256=%s bytes=%d lines=%d]\n%s\n", p, sha, bytes, lines, middleOut(c))
				blocks[i] = bm
			}
		}
		m["content"] = blocks
	}
}

func layer2(messages *[]map[string]any) *compactEvent {
	ms := *messages
	if len(ms) <= snipAfter {
		return nil
	}
	head := snipKeepHead
	tail := len(ms) - snipKeepNewest
	if tail <= head {
		return nil
	}
	dropped := tail - head
	next := append([]map[string]any{}, ms[:head]...)
	next = append(next, map[string]any{"role": "user", "content": []any{
		map[string]any{"type": "text", "text": fmt.Sprintf("[snipped %d messages; full log in session]", dropped)}}})
	next = append(next, ms[tail:]...)
	*messages = next
	return &compactEvent{Kind: "snip", Data: map[string]any{"dropped": dropped}}
}

func layer3(messages []map[string]any, warm bool) {
	start := 0
	if warm {
		start = len(messages) - microKeep
		if start < 0 {
			start = 0
		}
	}
	protect := len(messages) - microKeep
	for i, m := range messages {
		if i < start || i >= protect {
			continue
		}
		blocks := anySlice(m["content"])
		for j, b := range blocks {
			bm, _ := b.(map[string]any)
			if strv(bm, "type") != "tool_result" {
				continue
			}
			c := strv(bm, "content")
			if strings.HasPrefix(c, "[cleared") || strings.HasPrefix(c, "[tool_result persisted") {
				continue
			}
			bm["content"] = fmt.Sprintf("[cleared tool_result id=%s path=]", strv(bm, "tool_use_id"))
			blocks[j] = bm
		}
		m["content"] = blocks
	}
}

func layer4(messages []map[string]any) []map[string]any {
	var out []map[string]any
	i := 0
	for i < len(messages) {
		var paths []string
		j := i
		for j < len(messages) {
			var reads []string
			for _, b := range anySlice(messages[j]["content"]) {
				bm, _ := b.(map[string]any)
				if strv(bm, "type") == "tool_use" && strv(bm, "name") == "read" {
					inp, _ := bm["input"].(map[string]any)
					if p := strv(inp, "path"); p != "" {
						reads = append(reads, p)
					}
				}
			}
			if len(reads) == 0 {
				break
			}
			paths = append(paths, reads...)
			j++
		}
		if len(paths) >= 2 {
			out = append(out, map[string]any{"role": "user", "content": []any{
				map[string]any{"type": "text", "text": fmt.Sprintf("[%d reads: %s]", len(paths), strings.Join(paths, ", "))}}})
			i = j
		} else {
			out = append(out, messages[i])
			i++
		}
	}
	return out
}

func summarizeMsgs(messages []map[string]any, focus string) string {
	goal, last := "", ""
	var files []string
	for _, m := range messages {
		role := strv(m, "role")
		for _, b := range anySlice(m["content"]) {
			bm, _ := b.(map[string]any)
			if strv(bm, "type") == "text" && role == "user" && goal == "" {
				t := strv(bm, "text")
				if !strings.HasPrefix(t, "[") {
					if len(t) > 400 {
						t = t[:400]
					}
					goal = t
				}
			}
			if strv(bm, "type") == "text" && role == "assistant" {
				t := strv(bm, "text")
				if len(t) > 400 {
					t = t[:400]
				}
				last = t
			}
			if strv(bm, "type") == "tool_use" {
				inp, _ := bm["input"].(map[string]any)
				if p := strv(inp, "path"); p != "" {
					files = append(files, p)
				}
			}
		}
	}
	focusLine := ""
	if focus != "" {
		focusLine = "focus: " + focus + "\n"
	}
	return fmt.Sprintf("# World\n%sgoal: %s\nstatus: working\ndecisions: \nfiles_touched: %s\ntests: \nopen_questions: \n# Plan\n\n# Next\n%s\n",
		focusLine, goal, strings.Join(files, ", "), last)
}

func layer5(messages *[]map[string]any, keepTail int, focus string) (string, compactEvent) {
	world := summarizeMsgs(*messages, focus)
	ms := *messages
	if keepTail > len(ms) {
		keepTail = len(ms)
	}
	tail := append([]map[string]any{}, ms[len(ms)-keepTail:]...)
	next := []map[string]any{{"role": "user", "content": []any{
		map[string]any{"type": "text", "text": "[compact world]\n" + world}}}}
	next = append(next, tail...)
	*messages = next
	return world, compactEvent{Kind: "compact", Data: map[string]any{"reason": "l5", "focus": focus}}
}

func shouldL5(tokens, context, reserved int, warm bool, ratio float64) bool {
	if context == 0 {
		return false
	}
	thresh := int(float64(context) * ratio)
	if alt := context - reserved - 8000; alt < thresh {
		thresh = alt
	}
	if tokens >= thresh {
		return true
	}
	return !warm && tokens >= context/2
}

func compactMsgs(messages *[]map[string]any, session, home string, largeRead, context, boot int, warm, force bool, focus string, keepTail, lastL5Before, lastL5After int) compactOutcome {
	if keepTail <= 0 {
		keepTail = 6
	}
	before := estimateTokens(*messages, boot)
	var events []compactEvent
	layer1(*messages, home, largeRead)
	if snip := layer2(messages); snip != nil {
		events = append(events, *snip)
	}
	layer3(*messages, warm)
	mid := estimateTokens(*messages, boot)
	if force || shouldL5(mid, context, 8000, warm, 0.8) {
		if lastL5Before > 0 && float64(lastL5After) > float64(lastL5Before)*(1-thrashMinReduction) {
			events = append(events, compactEvent{Kind: "compact_thrash", Data: map[string]any{
				"before": lastL5Before, "after": lastL5After}})
			return compactOutcome{Events: events, Before: before, After: mid}
		}
		world, ev := layer5(messages, keepTail, focus)
		dest := filepath.Join(home, "world", session+".md")
		os.MkdirAll(filepath.Dir(dest), 0o755)
		os.WriteFile(dest, []byte(world), 0o644)
		events = append(events, ev)
		after := estimateTokens(*messages, boot)
		reason := "auto"
		if force {
			reason = "operator"
		}
		events = append(events, compactEvent{Kind: "compaction", Data: map[string]any{
			"before_tokens": before, "after_tokens": after, "reason": reason}})
		return compactOutcome{Events: events, World: world, Before: before, After: after}
	}
	return compactOutcome{Events: events, Before: before, After: estimateTokens(*messages, boot)}
}

func callHash(name string, input any) string {
	h := sha256.New()
	h.Write([]byte(name))
	b, _ := json.Marshal(input)
	h.Write(b)
	return hex.EncodeToString(h.Sum(nil))
}
