package main

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestCatalogPinsChatGPTThenDevin(t *testing.T) {
	ps := connectableProviders()
	if len(ps) < 4 {
		t.Fatalf("too few providers: %d", len(ps))
	}
	if ps[0].ID != "chatgpt" || ps[0].Kind != "chatgpt" {
		t.Fatalf("first provider %s/%s", ps[0].ID, ps[0].Kind)
	}
	if ps[1].ID != "devin" || ps[1].Kind != "devin" {
		t.Fatalf("second provider %s/%s", ps[1].ID, ps[1].Kind)
	}
	if catalogProvider("devin") == nil || catalogProvider("openrouter") == nil {
		t.Fatal("catalog missing Devin or OpenRouter")
	}
	if catalogProvider("devin").Models[0].ID != "swe-2-max" {
		t.Fatalf("expected swe-2-max first, got %s", catalogProvider("devin").Models[0].ID)
	}
}

func TestParseDevinList(t *testing.T) {
	raw := []byte(`{"families":[{"variants":[{"model_uid":"swe-1-7-medium","label":"SWE-1.7 Medium"},{"model_uid":"adaptive","label":"Adaptive"}]}]}`)
	ms, err := parseDevinList(raw)
	if err != nil {
		t.Fatal(err)
	}
	if len(ms) != 2 || ms[0].ID != "swe-1-7-medium" || ms[1].ID != "adaptive" {
		t.Fatalf("%+v", ms)
	}
}

func TestParseDevinHTTPModelsAndConnectFrames(t *testing.T) {
	ms := parseDevinCLIModels(map[string]any{
		"clientModelConfigs": []any{
			map[string]any{"label": "SWE-2 Max", "modelUid": "swe-2-max"},
			map[string]any{"label": "skip", "modelUid": "x", "disabled": true},
			map[string]any{"modelUid": "bare"},
		},
	})
	if len(ms) != 3 || ms[0].ID != "swe-2-max" || ms[1].ID != "x" || ms[2].ID != "bare" || ms[2].Name != "bare" {
		t.Fatalf("%+v", ms)
	}
	payload := []byte(`{"deltaText":"PONG"}`)
	buf := encodeConnectFrame(payload)
	frames, err := decodeConnectJSON(buf)
	if err != nil {
		t.Fatal(err)
	}
	if len(frames) != 1 || strv(frames[0], "deltaText") != "PONG" {
		t.Fatalf("%+v", frames)
	}
}

func TestAvailableModelsShowCatalogWhenProviderInConfig(t *testing.T) {
	home := t.TempDir()
	t.Setenv("COSMOS_CLI_HOME", home)
	t.Setenv("COSMOS_CLI_KEYRING", "0")
	if err := os.WriteFile(filepath.Join(home, "config.toml"), []byte(
		"[providers.openai]\nkind = \"responses\"\nbase_url = \"https://api.openai.com/v1\"\napi_key_env = \"OPENAI_API_KEY\"\n",
	), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := storeKey("openai", "sk-test"); err != nil {
		t.Fatal(err)
	}
	cfg := loadConfig(home)
	models := cfg.availableModels("")
	found := false
	for _, m := range models {
		if strings.HasPrefix(m.alias, "openai/") {
			found = true
			break
		}
	}
	if !found {
		t.Fatalf("catalog openai models missing when [providers.openai] exists: %+v", models)
	}
}

func TestProviderForCatalogRef(t *testing.T) {
	home := t.TempDir()
	t.Setenv("COSMOS_CLI_HOME", home)
	t.Setenv("COSMOS_CLI_KEYRING", "0")
	cfg := loadConfig(home)
	pcfg, ok := cfg.providerFor("devin/swe-1-7-medium")
	if !ok || pcfg.Kind != "devin" || pcfg.Model != "swe-1-7-medium" {
		t.Fatalf("devin ref: ok=%v %+v", ok, pcfg)
	}
	pcfg, ok = cfg.providerFor("devin/swe-2-max")
	if !ok || pcfg.Kind != "devin" || pcfg.Model != "swe-2-max" {
		t.Fatalf("swe-2-max ref: ok=%v %+v", ok, pcfg)
	}
	pcfg, ok = cfg.providerFor("anthropic/claude-opus-5-5")
	if !ok || pcfg.Kind != "anthropic" {
		t.Fatalf("anthropic ref: ok=%v %+v", ok, pcfg)
	}
}
