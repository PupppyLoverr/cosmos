// config — COSMOS_CLI_HOME/config.toml. Minimal TOML parser covering the
// contract's subset: [section], [a.b], [[array-of-tables]], key = "str"|int|
// float|bool|["a","b"]. Enough for config.example.toml.
package main

import (
	"fmt"
	"os"
	"path/filepath"
	"sort"
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
	UserName     string
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
		cfg.UserName = strv(t, "user_name")
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
		if cfg.DefaultModel == "" {
			cfg.DefaultModel = "fixture"
		}
	}
	return cfg
}

func (c *Config) providerFor(alias string) (ProviderCfg, bool) {
	if m, ok := c.Models[alias]; ok {
		p, ok := c.Providers[m.Provider]
		if !ok {
			return ProviderCfg{}, false
		}
		return ProviderCfg{
			Kind: p["kind"], BaseURL: p["base_url"], APIKeyEnv: p["api_key_env"],
			Model: m.ID, Provider: m.Provider, Context: uint64(m.Context), Prices: m.Price,
		}, true
	}
	pid, mid, ok := catalogSplitRef(alias)
	if !ok {
		return ProviderCfg{}, false
	}
	if p, ok := c.Providers[pid]; ok {
		ctx := uint64(128000)
		price := map[string]float64{}
		if def := catalogProvider(pid); def != nil {
			if m := def.model(mid); m != nil {
				ctx = m.Context
				price = map[string]float64{"input": m.Price[0], "output": m.Price[1], "cache_read": m.Price[2], "cache_write": m.Price[3]}
			}
		}
		if live := liveCached(pid); live != nil {
			for _, m := range live {
				if m.ID == mid {
					ctx = m.Context
					price = map[string]float64{"input": m.Price[0], "output": m.Price[1], "cache_read": m.Price[2], "cache_write": m.Price[3]}
				}
			}
		}
		return ProviderCfg{
			Kind: p["kind"], BaseURL: p["base_url"], APIKeyEnv: p["api_key_env"],
			Model: mid, Provider: pid, Context: ctx, Prices: price,
		}, true
	}
	def := catalogProvider(pid)
	if def == nil {
		return ProviderCfg{}, false
	}
	ctx := uint64(128000)
	price := map[string]float64{}
	if m := def.model(mid); m != nil {
		ctx = m.Context
		price = map[string]float64{"input": m.Price[0], "output": m.Price[1], "cache_read": m.Price[2], "cache_write": m.Price[3]}
	}
	if live := liveCached(pid); live != nil {
		for _, m := range live {
			if m.ID == mid {
				ctx = m.Context
				price = map[string]float64{"input": m.Price[0], "output": m.Price[1], "cache_read": m.Price[2], "cache_write": m.Price[3]}
			}
		}
	}
	env := ""
	if len(def.Env) > 0 {
		env = def.Env[0]
	}
	return ProviderCfg{
		Kind: def.Kind, BaseURL: def.BaseURL, APIKeyEnv: env,
		Model: mid, Provider: pid, Context: ctx, Prices: price,
	}, true
}

func tomlQuote(s string) string {
	var b strings.Builder
	b.WriteByte('"')
	for _, r := range s {
		switch r {
		case 0:
			continue
		case '\b':
			b.WriteString(`\b`)
		case '\t':
			b.WriteString(`\t`)
		case '\n':
			b.WriteString(`\n`)
		case '\f':
			b.WriteString(`\f`)
		case '\r':
			b.WriteString(`\r`)
		case '"':
			b.WriteString(`\"`)
		case '\\':
			b.WriteString(`\\`)
		default:
			if r < 0x20 {
				fmt.Fprintf(&b, `\u%04x`, r)
			} else {
				b.WriteRune(r)
			}
		}
	}
	b.WriteByte('"')
	return b.String()
}

func sanitizeTopLevel(s string) string {
	s = strings.TrimSpace(s)
	var b strings.Builder
	for _, r := range s {
		if r == 0 || (r < 0x20 && r != '\t') {
			continue
		}
		b.WriteRune(r)
	}
	return b.String()
}

func setTopLevel(home, key, value string) error {
	file := filepath.Join(home, "config.toml")
	text := ""
	if b, err := os.ReadFile(file); err == nil {
		text = string(b)
	}
	value = sanitizeTopLevel(value)
	line := fmt.Sprintf("%s = %s", key, tomlQuote(value))
	var out []string
	done := false
	inTable := false
	prefix := key + " "
	eq := key + "="
	for _, l := range strings.Split(text, "\n") {
		t := strings.TrimLeft(l, " \t")
		if strings.HasPrefix(t, "[") {
			inTable = true
		}
		if !inTable && !done && (strings.HasPrefix(t, prefix) && strings.Contains(t, "=") || strings.HasPrefix(t, eq)) {
			out = append(out, line)
			done = true
			continue
		}
		out = append(out, l)
	}
	for len(out) > 0 && out[len(out)-1] == "" {
		out = out[:len(out)-1]
	}
	if !done {
		out = append([]string{line}, out...)
	}
	if err := os.MkdirAll(home, 0o755); err != nil {
		return err
	}
	return os.WriteFile(file, []byte(strings.Join(out, "\n")+"\n"), 0o644)
}

func setUserName(home, name string) error {
	return setTopLevel(home, "user_name", name)
}

func setDefaultModel(home, alias string) error {
	return setTopLevel(home, "default_model", alias)
}

func ensureProvider(home string, p welcomeProv) error {
	file := filepath.Join(home, "config.toml")
	text := ""
	if b, err := os.ReadFile(file); err == nil {
		text = string(b)
	}
	if strings.Contains(text, "[providers."+p.id+"]") {
		return nil
	}
	block := fmt.Sprintf("\n[providers.%s]\nkind = %q\nbase_url = %q\napi_key_env = %q\n",
		p.id, p.kind, p.base, p.env)
	if err := os.MkdirAll(home, 0o755); err != nil {
		return err
	}
	return os.WriteFile(file, []byte(strings.TrimRight(text, "\n")+block), 0o644)
}

func ensureModel(home string, p welcomeProv, id string) error {
	file := filepath.Join(home, "config.toml")
	text := ""
	if b, err := os.ReadFile(file); err == nil {
		text = string(b)
	}
	alias := p.id + "-" + id
	if strings.Contains(text, "[models."+alias+"]") {
		return nil
	}
	block := fmt.Sprintf("\n[models.%s]\nprovider = %q\nid = %q\ncontext = 128000\n",
		alias, p.id, id)
	if err := os.MkdirAll(home, 0o755); err != nil {
		return err
	}
	return os.WriteFile(file, []byte(strings.TrimRight(text, "\n")+block), 0o644)
}

func (c *Config) allowedHosts() []string {
	var out []string
	seen := map[string]bool{}
	add := func(h string) {
		if h != "" && !seen[h] {
			seen[h] = true
			out = append(out, h)
		}
	}
	for _, p := range c.Providers {
		add(catalogHostOf(p["base_url"]))
	}
	for _, p := range catalogProviders() {
		add(catalogHostOf(p.BaseURL))
	}
	add("chatgpt.com")
	add("auth.openai.com")
	add("server.codeium.com")
	add("inference.codeium.com")
	add("api.devin.ai")
	return out
}

func providerReady(p CatalogProvider) bool {
	switch p.Kind {
	case "chatgpt":
		return false // ChatGPT OAuth lives in zcli/zdev
	case "devin":
		return devinKnownReady()
	}
	if len(p.Env) == 0 {
		return false
	}
	return credStored(p.ID) != "" || (p.Env[0] != "" && strings.TrimSpace(os.Getenv(p.Env[0])) != "")
}

type modelChoice struct {
	alias, provider, name, id, kind string
	context                         uint64
	current                         bool
}

func (c *Config) availableModels(current string) []modelChoice {
	var out []modelChoice
	seen := map[string]bool{}
	var aliases []string
	for alias := range c.Models {
		aliases = append(aliases, alias)
	}
	sort.Strings(aliases)
	for _, alias := range aliases {
		m := c.Models[alias]
		kind := ""
		if p, ok := c.Providers[m.Provider]; ok {
			kind = p["kind"]
		}
		out = append(out, modelChoice{
			alias: alias, provider: m.Provider, name: alias, id: m.ID, kind: kind,
			context: uint64(m.Context), current: alias == current,
		})
		seen[alias] = true
	}
	for _, p := range catalogProviders() {
		if !providerReady(p) {
			continue
		}
		models := liveCached(p.ID)
		if len(models) == 0 {
			models = p.Models
		}
		for _, m := range models {
			alias := p.ID + "/" + m.ID
			if seen[alias] {
				continue
			}
			seen[alias] = true
			out = append(out, modelChoice{
				alias: alias, provider: p.ID, name: m.Name, id: m.ID, kind: p.Kind,
				context: m.Context, current: alias == current,
			})
		}
	}
	return out
}
