// config — COSMOS_CLI_HOME/config.toml. Minimal TOML parser covering the
// contract's subset: [section], [a.b], [[array-of-tables]], key = "str"|int|
// float|bool|["a","b"]. Enough for config.example.toml.
package main

import (
	"os"
	"path/filepath"
	"strconv"
	"strings"
)

type tomlTable = map[string]any

func parseTOML(text string) tomlTable {
	root := tomlTable{}
	cur := root
	for _, raw := range strings.Split(text, "\n") {
		line := strings.TrimSpace(raw)
		if line == "" || strings.HasPrefix(line, "#") {
			continue
		}
		if strings.HasPrefix(line, "[[") && strings.HasSuffix(line, "]]") {
			path := strings.Split(strings.Trim(line, "[]"), ".")
			// descend, last element is an array of tables
			parent := root
			for _, p := range path[:len(path)-1] {
				next, ok := parent[p].(tomlTable)
				if !ok {
					next = tomlTable{}
					parent[p] = next
				}
				parent = next
			}
			key := path[len(path)-1]
			arr, _ := parent[key].([]tomlTable)
			arr = append(arr, tomlTable{})
			parent[key] = arr
			cur = arr[len(arr)-1]
			continue
		}
		if strings.HasPrefix(line, "[") && strings.HasSuffix(line, "]") {
			path := strings.Split(strings.Trim(line, "[]"), ".")
			cur = root
			for _, p := range path {
				next, ok := cur[p].(tomlTable)
				if !ok {
					next = tomlTable{}
					cur[p] = next
				}
				cur = next
			}
			continue
		}
		if k, v, ok := strings.Cut(line, "="); ok {
			cur[strings.TrimSpace(k)] = parseValue(strings.TrimSpace(v))
		}
	}
	return root
}

func parseValue(v string) any {
	if strings.HasPrefix(v, "\"") {
		// Quoted string: take up to the closing quote; a trailing # comment may follow.
		if end := strings.Index(v[1:], "\""); end >= 0 {
			return v[1 : 1+end]
		}
		return strings.Trim(v, "\"")
	}
	if i := strings.Index(v, "#"); i > 0 {
		v = strings.TrimSpace(v[:i])
	}
	if strings.HasPrefix(v, "[") && strings.HasSuffix(v, "]") {
		inner := strings.Trim(v, "[]")
		if strings.TrimSpace(inner) == "" {
			return []any{}
		}
		var out []any
		for _, part := range strings.Split(inner, ",") {
			out = append(out, parseValue(strings.TrimSpace(part)))
		}
		return out
	}
	if v == "true" || v == "false" {
		return v == "true"
	}
	if f, err := strconv.ParseFloat(v, 64); err == nil {
		return f
	}
	return v
}

func strSlice(v any) []string {
	var out []string
	if arr, ok := v.([]any); ok {
		for _, x := range arr {
			if s, ok := x.(string); ok {
				out = append(out, s)
			}
		}
	}
	return out
}

type Config struct {
	DefaultModel string
	Providers    map[string]map[string]string // name → kind/base_url/api_key_env
	Models       map[string]ModelEntry
	PermAllow    []string
	PermAsk      []string
	MaxParallel  int
	LargeRead    int
	Hooks        []Hook
}

type ModelEntry struct {
	Provider, ID string
	Context      float64
	Price        map[string]float64
}

type Hook struct {
	Event, Matcher, Command string
}

func loadConfig(home string) *Config {
	cfg := &Config{
		Providers:   map[string]map[string]string{},
		Models:      map[string]ModelEntry{},
		MaxParallel: 3,
		LargeRead:   200,
	}
	data, err := os.ReadFile(filepath.Join(home, "config.toml"))
	if err == nil {
		t := parseTOML(string(data))
		cfg.DefaultModel = strv(t, "default_model")
		if ps, ok := t["providers"].(tomlTable); ok {
			for name, v := range ps {
				if pt, ok := v.(tomlTable); ok {
					cfg.Providers[name] = map[string]string{
						"kind":        strv(pt, "kind"),
						"base_url":    strv(pt, "base_url"),
						"api_key_env": strv(pt, "api_key_env"),
					}
				}
			}
		}
		if ms, ok := t["models"].(tomlTable); ok {
			for alias, v := range ms {
				if mt, ok := v.(tomlTable); ok {
					price := map[string]float64{}
					if pt, ok := mt["price"].(tomlTable); ok {
						for k, pv := range pt {
							price[k], _ = pv.(float64)
						}
					}
					cfg.Models[alias] = ModelEntry{
						Provider: strv(mt, "provider"),
						ID:       strv(mt, "id"),
						Context:  numv(mt, "context"),
						Price:    price,
					}
				}
			}
		}
		if perms, ok := t["permissions"].(tomlTable); ok {
			cfg.PermAllow = strSlice(perms["allow"])
			cfg.PermAsk = strSlice(perms["ask"])
		}
		if sa, ok := t["subagents"].(tomlTable); ok {
			if v := numv(sa, "max_parallel"); v > 0 {
				cfg.MaxParallel = int(v)
			}
		}
		if br, ok := t["bridge"].(tomlTable); ok {
			if v := numv(br, "large_read_lines"); v > 0 {
				cfg.LargeRead = int(v)
			}
		}
		if hooks, ok := t["hooks"].([]tomlTable); ok {
			for _, h := range hooks {
				cfg.Hooks = append(cfg.Hooks, Hook{
					Event:   strv(h, "event"),
					Matcher: strv(h, "matcher"),
					Command: strv(h, "command"),
				})
			}
		}
	}
	if len(cfg.Providers) == 0 {
		cfg.Providers["fixture"] = map[string]string{"kind": "fixture"}
	}
	if len(cfg.Models) == 0 {
		cfg.Models["fixture"] = ModelEntry{
			Provider: "fixture", ID: "fixture", Context: 128000,
			Price: map[string]float64{},
		}
		cfg.DefaultModel = "fixture"
	}
	return cfg
}

func (c *Config) providerFor(alias string) (ProviderCfg, bool) {
	m, ok := c.Models[alias]
	if !ok {
		return ProviderCfg{}, false
	}
	p, ok := c.Providers[m.Provider]
	if !ok {
		return ProviderCfg{}, false
	}
	return ProviderCfg{
		Kind: p["kind"], BaseURL: p["base_url"], APIKeyEnv: p["api_key_env"],
		Model: m.ID, Context: uint64(m.Context), Prices: m.Price,
	}, true
}

func (c *Config) allowedHosts() []string {
	var out []string
	for _, p := range c.Providers {
		u := p["base_url"]
		if i := strings.Index(u, "://"); i >= 0 {
			host := u[i+3:]
			if j := strings.IndexAny(host, "/:?"); j >= 0 {
				host = host[:j]
			}
			if host != "" {
				out = append(out, host)
			}
		}
	}
	return out
}
