package main

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	tea "github.com/charmbracelet/bubbletea"
)

func TestOnboardingMarker(t *testing.T) {
	home := t.TempDir()
	t.Setenv("COSMOS_CLI_HOME", home)
	t.Setenv("ZAGY_NO_ONBOARDING", "")
	t.Setenv("ZCLI_NO_ONBOARDING", "")
	os.Unsetenv("ZAGY_NO_ONBOARDING")
	os.Unsetenv("ZCLI_NO_ONBOARDING")
	if !welcomeNeeded() {
		t.Fatal("fresh home should need onboarding")
	}
	markOnboarded()
	if welcomeNeeded() {
		t.Fatal("marker should skip onboarding")
	}
	t.Setenv("ZAGY_NO_ONBOARDING", "1")
	_ = os.Remove(onboardMarkerPath())
	if welcomeNeeded() {
		t.Fatal("ZAGY_NO_ONBOARDING should skip")
	}
}

func TestSetUserNameRoundTrip(t *testing.T) {
	home := t.TempDir()
	t.Setenv("COSMOS_CLI_HOME", home)
	if err := setUserName(home, "Rav"); err != nil {
		t.Fatal(err)
	}
	cfg := loadConfig(home)
	if cfg.UserName != "Rav" {
		t.Fatalf("user_name %q", cfg.UserName)
	}
	if err := setUserName(home, "Rav\x00\x00"); err != nil {
		t.Fatal(err)
	}
	text, _ := os.ReadFile(filepath.Join(home, "config.toml"))
	if strings.Contains(string(text), `\x`) {
		t.Fatalf("wrote invalid TOML escape: %s", text)
	}
	if loadConfig(home).UserName != "Rav" {
		t.Fatalf("nul name %q", loadConfig(home).UserName)
	}
	if configUserName() != "Rav" {
		t.Fatalf("configUserName %q", configUserName())
	}
}

func TestCredsRoundTrip(t *testing.T) {
	home := t.TempDir()
	t.Setenv("COSMOS_CLI_HOME", home)
	t.Setenv("COSMOS_CLI_KEYRING", "0")
	if err := storeKey("openrouter", "sk-test-key"); err != nil {
		t.Fatal(err)
	}
	if credStored("openrouter") != "sk-test-key" {
		t.Fatal("stored key missing")
	}
	p := welcomeProv{id: "openrouter", name: "OpenRouter", env: "OPENROUTER_API_KEY",
		kind: "openai", base: "https://openrouter.ai/api/v1"}
	if err := ensureProvider(home, p); err != nil {
		t.Fatal(err)
	}
	if err := ensureModel(home, p, "auto"); err != nil {
		t.Fatal(err)
	}
	cfg := loadConfig(home)
	if cfg.Providers["openrouter"]["kind"] != "openai" {
		t.Fatalf("provider %v", cfg.Providers["openrouter"])
	}
	if _, ok := cfg.Models["openrouter-auto"]; !ok {
		t.Fatalf("models %v", cfg.Models)
	}
	pcfg, ok := cfg.providerFor("openrouter-auto")
	if !ok {
		t.Fatal("providerFor")
	}
	if apiKey(pcfg) != "sk-test-key" {
		t.Fatalf("apiKey %q", apiKey(pcfg))
	}
}

func TestWelcomePhases(t *testing.T) {
	home := t.TempDir()
	t.Setenv("COSMOS_CLI_HOME", home)
	f := newWelcome(80, 24)
	if f.phase != wWarp {
		t.Fatal("start at warp")
	}
	f.onKey(tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune("x")})
	if f.phase != wName {
		t.Fatalf("any key skips intro → name, got %v", f.phase)
	}
	f.name = "Rav"
	f.onKey(tea.KeyMsg{Type: tea.KeyEnter})
	if f.phase != wKeys {
		t.Fatalf("enter → keys, got %v", f.phase)
	}
	kv := f.View()
	for _, want := range []string{"Devin", "OpenRouter", "ChatGPT"} {
		if !strings.Contains(kv, want) {
			t.Fatalf("keys screen missing %q: %q", want, kv)
		}
	}
	order := f.filteredProvs()
	f.sel = len(order) // Continue
	f.onKey(tea.KeyMsg{Type: tea.KeyEnter})
	if f.phase != wReady {
		t.Fatalf("continue → ready, got %v", f.phase)
	}
	v := f.View()
	if !strings.Contains(v, "You're all set, Rav.") {
		t.Fatalf("ready missing greeting: %q", v)
	}
	if !strings.Contains(v, "welcome to") && !stringsContainsAny(v, "cosmos", "zagy") {
		// ready screen is how-to; title lives on name. Check how-to copy.
	}
	if !strings.Contains(v, "shift+tab") || !strings.Contains(v, "launch zagy") {
		t.Fatalf("ready how-to missing: %q", v)
	}
	f.onKey(tea.KeyMsg{Type: tea.KeyEnter})
	if !f.done {
		t.Fatal("enter launches")
	}
	if configUserName() != "Rav" {
		t.Fatal("name not persisted")
	}
	if _, err := os.Stat(filepath.Join(home, ".zagy-onboarded")); err != nil {
		t.Fatal("marker missing")
	}
}

func TestWelcomeNameScreenHasGalaxyTitle(t *testing.T) {
	home := t.TempDir()
	t.Setenv("COSMOS_CLI_HOME", home)
	f := newWelcome(100, 30)
	f.phase = wName
	v := f.View()
	if !strings.Contains(v, welcomeQuestion) {
		t.Fatalf("name prompt missing: %q", v)
	}
	if !strings.Contains(v, welcomeSubtitle) {
		t.Fatalf("subtitle missing: %q", v)
	}
}

func stringsContainsAny(s string, parts ...string) bool {
	for _, p := range parts {
		if strings.Contains(s, p) {
			return true
		}
	}
	return false
}
