// Tool-call repair. Same algorithm as ztools/src/repair.rs — keep in lockstep.
package main

import (
	"encoding/json"
	"fmt"
	"strconv"
	"strings"
)

type Repair struct {
	From   string
	To     string
	Reason string
}

type RepairResult struct {
	Name    string
	Input   map[string]any
	Repairs []Repair
	Reject  string
}

var toolAlias = [][2]string{
	{"Read", "read"}, {"read_file", "read"}, {"file_read", "read"},
	{"Write", "write"}, {"write_file", "write"}, {"file_write", "write"},
	{"Edit", "edit"}, {"StrReplace", "edit"}, {"str_replace", "edit"},
	{"Bash", "bash"}, {"Shell", "bash"}, {"shell", "bash"},
	{"Grep", "grep"}, {"Glob", "glob"}, {"Ls", "ls"}, {"List", "ls"},
	{"MemorySearch", "memory_search"}, {"memory.search", "memory_search"},
	{"MemoryWrite", "memory_write"}, {"Task", "task"}, {"TodoWrite", "task"},
}

func remapName(name string) (string, *Repair) {
	trimmed := strings.TrimSpace(name)
	for _, pair := range toolAlias {
		from, to := pair[0], pair[1]
		if trimmed == from || (strings.EqualFold(trimmed, from) && from != to) {
			if trimmed != to {
				return to, &Repair{From: trimmed, To: to, Reason: "alias"}
			}
		}
	}
	var specs []map[string]any
	json.Unmarshal([]byte(ToolsJSON), &specs)
	for _, s := range specs {
		n := strv(s, "name")
		if strings.EqualFold(trimmed, n) && trimmed != n {
			return n, &Repair{From: trimmed, To: n, Reason: "case"}
		}
	}
	return trimmed, nil
}

func unwrapJSON(raw string) string {
	s := strings.TrimSpace(raw)
	if strings.HasPrefix(s, "```json") {
		s = strings.TrimPrefix(s, "```json")
	} else if strings.HasPrefix(s, "```") {
		s = strings.TrimPrefix(s, "```")
	}
	s = strings.TrimSpace(s)
	if i := strings.LastIndex(s, "```"); i >= 0 {
		s = strings.TrimSpace(s[:i])
	}
	a := strings.Index(s, "{")
	b := strings.LastIndex(s, "}")
	if a >= 0 && b > a {
		return s[a : b+1]
	}
	if a >= 0 {
		body := s[a:]
		opens := strings.Count(body, "{")
		closes := strings.Count(body, "}")
		for i := 0; i < opens-closes; i++ {
			body += "}"
		}
		return body
	}
	return s
}

func repairTool(name string, input any) RepairResult {
	var repairs []Repair
	n, alias := remapName(name)
	if alias != nil {
		repairs = append(repairs, *alias)
	}
	obj := map[string]any{}
	switch v := input.(type) {
	case map[string]any:
		for k, val := range v {
			obj[k] = val
		}
	case string:
		var parsed map[string]any
		if json.Unmarshal([]byte(unwrapJSON(v)), &parsed) == nil {
			obj = parsed
			repairs = append(repairs, Repair{From: "string", To: "object", Reason: "unwrap"})
		}
	}
	var specs []map[string]any
	json.Unmarshal([]byte(ToolsJSON), &specs)
	var spec map[string]any
	for _, s := range specs {
		if strv(s, "name") == n {
			spec = s
			break
		}
	}
	if spec != nil {
		schema, _ := spec["input_schema"].(map[string]any)
		props, _ := schema["properties"].(map[string]any)
		known := map[string]bool{}
		for k := range props {
			known[k] = true
		}
		for k := range obj {
			if !known[k] {
				delete(obj, k)
				repairs = append(repairs, Repair{From: k, To: "", Reason: "unknown_key"})
			}
		}
		for k, pv := range props {
			pm, _ := pv.(map[string]any)
			ty := strv(pm, "type")
			v, ok := obj[k]
			if !ok {
				continue
			}
			switch ty {
			case "integer":
				if s, ok := v.(string); ok {
					if i, err := strconv.Atoi(strings.TrimSpace(s)); err == nil {
						repairs = append(repairs, Repair{From: k + ":" + s, To: k + ":" + strconv.Itoa(i), Reason: "coerce"})
						obj[k] = i
					}
				} else if f, ok := v.(float64); ok {
					obj[k] = int(f)
				}
			case "boolean":
				if s, ok := v.(string); ok {
					b := s == "true" || s == "1" || s == "yes"
					repairs = append(repairs, Repair{From: k + ":" + s, To: k + ":" + strconv.FormatBool(b), Reason: "coerce"})
					obj[k] = b
				}
			case "string":
				if _, ok := v.(string); !ok {
					s := fmt.Sprint(v)
					repairs = append(repairs, Repair{From: k, To: s, Reason: "coerce"})
					obj[k] = s
				}
			}
		}
		if req, ok := schema["required"].([]any); ok {
			for _, r := range req {
				key, _ := r.(string)
				v, exists := obj[key]
				if !exists || v == nil || v == "" {
					return RepairResult{Name: n, Input: obj, Repairs: repairs, Reject: "repair: missing field " + key + " — resend"}
				}
			}
		}
	}
	return RepairResult{Name: n, Input: obj, Repairs: repairs}
}
