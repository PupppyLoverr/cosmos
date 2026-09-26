// Built-in provider catalog — the same catalog.json zcli embeds
// (rust-tui/crates/zcore/catalog, generated from models.dev), plus ChatGPT,
// Devin, 9Router and local servers by hand.
package main

import (
	_ "embed"
	"encoding/json"
	"fmt"
	"sort"
	"strings"
	"sync"
)

//go:embed catalog.json
var catalogJSON []byte

type CatalogModel struct {
	ID        string     `json:"id"`
	Name      string     `json:"name"`
	Context   uint64     `json:"context"`
	Output    uint64     `json:"output"`
	Reasoning bool       `json:"reasoning"`
	Price     [4]float64 `json:"price"`
}

type CatalogProvider struct {
	ID      string         `json:"id"`
	Name    string         `json:"name"`
	Kind    string         `json:"kind"`
	BaseURL string         `json:"base_url"`
	Env     []string       `json:"env"`
	Models  []CatalogModel `json:"models"`
}

func (p CatalogProvider) isSubscription() bool {
	return p.Kind == "chatgpt" || p.Kind == "devin"
}

func (p CatalogProvider) model(id string) *CatalogModel {
	for i := range p.Models {
		if p.Models[i].ID == id {
			return &p.Models[i]
		}
	}
	return nil
}

func catalogHostOf(url string) string {
	rest := url
	if i := strings.Index(url, "://"); i >= 0 {
		rest = url[i+3:]
	}
	parts := strings.FieldsFunc(rest, func(r rune) bool { return r == '/' || r == ':' || r == '?' })
	if len(parts) == 0 {
		return ""
	}
	return parts[0]
}

func cm(id, name string, ctx, out uint64, reasoning bool) CatalogModel {
	return CatalogModel{ID: id, Name: name, Context: ctx, Output: out, Reasoning: reasoning}
}

func catalogChatGPT() CatalogProvider {
	return CatalogProvider{
		ID: "chatgpt", Name: "ChatGPT (Plus / Pro subscription)", Kind: "chatgpt",
		BaseURL: "https://chatgpt.com/backend-api",
		Models: []CatalogModel{
			cm("gpt-5.5", "GPT-5.5", 272000, 128000, true),
			cm("gpt-6-astra", "GPT-6 Astra", 272000, 128000, true),
			cm("gpt-5.6-sol", "GPT-5.6 Sol", 272000, 128000, true),
			cm("gpt-5.6-terra", "GPT-5.6 Terra", 272000, 128000, true),
			cm("gpt-5.6-luna", "GPT-5.6 Luna", 272000, 128000, true),
			cm("gpt-5.3-codex-spark", "GPT-5.3 Codex Spark", 128000, 128000, true),
		},
	}
}

func catalogDevin() CatalogProvider {
	return CatalogProvider{
		ID: "devin", Name: "Devin (subscription)", Kind: "devin",
		BaseURL: "https://server.codeium.com",
		Models: []CatalogModel{
			cm("swe-2-max", "SWE-2 Max", 200000, 32000, true),
			cm("swe-2-high", "SWE-2 High", 200000, 32000, true),
			cm("swe-2-medium", "SWE-2 Medium", 200000, 32000, true),
			cm("swe-1-7-medium", "SWE-1.7 Medium", 200000, 32000, true),
			cm("claude-fable-5-1-high", "Claude Fable 5.1 High", 200000, 32000, true),
			cm("adaptive", "Adaptive", 200000, 32000, true),
		},
	}
}

func catalogRouter9() CatalogProvider {
	return CatalogProvider{
		ID: "router9", Name: "9Router", Kind: "openai",
		BaseURL: "http://localhost:20128/v1", Env: []string{"ROUTER9_API_KEY"},
		Models: []CatalogModel{
			cm("kr/claude-sonnet-4.5", "Claude Sonnet 4.5 (Kiro)", 200000, 32000, false),
			cm("ag/gemini-3.8-flash", "Gemini 3.8 Flash (Antigravity)", 1048576, 32000, false),
			cm("kc/kilo-auto/free", "Kilo Auto (free)", 200000, 32000, false),
		},
	}
}

var (
	catalogOnce sync.Once
	catalogAll  []CatalogProvider
)

func catalogProviders() []CatalogProvider {
	catalogOnce.Do(func() {
		var file struct {
			Providers []CatalogProvider `json:"providers"`
		}
		_ = json.Unmarshal(catalogJSON, &file)
		catalogAll = append(catalogAll, catalogChatGPT(), catalogDevin(), catalogRouter9())
		catalogAll = append(catalogAll, file.Providers...)
		catalogAll = append(catalogAll,
			CatalogProvider{ID: "ollama", Name: "Ollama (local)", Kind: "openai", BaseURL: "http://127.0.0.1:11434/v1"},
			CatalogProvider{ID: "lmstudio", Name: "LM Studio (local)", Kind: "openai", BaseURL: "http://127.0.0.1:1234/v1"},
		)
	})
	return catalogAll
}

func catalogProvider(id string) *CatalogProvider {
	all := catalogProviders()
	for i := range all {
		if all[i].ID == id {
			return &all[i]
		}
	}
	return nil
}

func catalogSplitRef(alias string) (string, string, bool) {
	i := strings.Index(alias, "/")
	if i <= 0 || i == len(alias)-1 {
		return "", "", false
	}
	return alias[:i], alias[i+1:], true
}

// Same pinned order as zcli-welcome / zdev: subscriptions first, then the
// labs people actually bring a key for.
var pinOrder = []struct{ id, pitch, url string }{
	{"chatgpt", "use your Plus / Pro plan — no API key", ""},
	{"devin", "your Devin subscription — Cosmos runs the tools", ""},
	{"openrouter", "one key, 300+ models from every lab", "https://openrouter.ai/keys"},
	{"router9", "your local 9Router — routes the subscriptions you have", "http://localhost:20128 → API keys"},
	{"anthropic", "Claude", "https://console.anthropic.com/settings/keys"},
	{"openai", "GPT and o-series", "https://platform.openai.com/api-keys"},
	{"google", "Gemini", "https://aistudio.google.com/apikey"},
	{"xai", "Grok", "https://console.x.ai"},
	{"meta", "Muse", ""},
	{"groq", "very fast open models", "https://console.groq.com/keys"},
	{"deepseek", "DeepSeek V and R", "https://platform.deepseek.com/api_keys"},
	{"mistral", "Mistral and Codestral", "https://console.mistral.ai/api-keys"},
	{"xiaomi", "MiMo", ""},
	{"moonshotai", "Kimi", "https://platform.moonshot.ai"},
	{"zai", "GLM", "https://z.ai/manage-apikey"},
	{"cerebras", "fastest inference", "https://cloud.cerebras.ai"},
	{"minimax", "MiniMax", ""},
}

func pinRank(id string) int {
	for i, p := range pinOrder {
		if p.id == id {
			return i
		}
	}
	return len(pinOrder)
}

func pitchFor(p CatalogProvider) string {
	for _, x := range pinOrder {
		if x.id == p.ID {
			return x.pitch
		}
	}
	if len(p.Env) > 0 {
		return p.Env[0]
	}
	return p.Kind
}

func pinURL(id string) string {
	for _, x := range pinOrder {
		if x.id == id {
			return x.url
		}
	}
	return ""
}

func catalogDisplayName(p CatalogProvider) string {
	switch p.Kind {
	case "chatgpt":
		return "ChatGPT"
	case "devin":
		return "Devin"
	default:
		return p.Name
	}
}

func connectableProviders() []CatalogProvider {
	var out []CatalogProvider
	for _, p := range catalogProviders() {
		if p.isSubscription() || len(p.Env) > 0 {
			out = append(out, p)
		}
	}
	sort.SliceStable(out, func(i, j int) bool {
		ri, rj := pinRank(out[i].ID), pinRank(out[j].ID)
		if ri != rj {
			return ri < rj
		}
		return out[i].Name < out[j].Name
	})
	return out
}

func filterProviders(q string) []CatalogProvider {
	q = strings.ToLower(strings.TrimSpace(q))
	all := connectableProviders()
	if q == "" {
		return all
	}
	var out []CatalogProvider
	for _, p := range all {
		hay := strings.ToLower(p.ID + " " + p.Name + " " + p.Kind + " " + pitchFor(p))
		if strings.Contains(hay, q) {
			out = append(out, p)
		}
	}
	return out
}

func providerStatus(p CatalogProvider) (ready bool, detail string) {
	switch p.Kind {
	case "chatgpt":
		return false, "sign in via zcli / zdev"
	case "devin":
		devinMu.Lock()
		c := devinCached
		devinMu.Unlock()
		if c != nil {
			return c.ready, c.detail
		}
		if n := len(liveCached("devin")); n > 0 {
			return true, fmt.Sprintf("cached · %d models", n)
		}
		if devinSignedIn() {
			return true, "enter to connect"
		}
		if devinExecutable() == "" {
			return false, "devin auth login"
		}
		return false, "enter to connect"
	}
	if providerReady(p) {
		if k := credStored(p.ID); k != "" {
			return true, maskKey(k) + " · stored"
		}
		return true, "env"
	}
	return false, fmt.Sprintf("%d models", len(p.Models))
}
