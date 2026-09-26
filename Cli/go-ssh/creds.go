// Credentials shared with zcli and zdev.
//
// An API key resolves from the provider's env var, then
// COSMOS_CLI_HOME/credentials.json (the family file). New keys go to that
// file so a key saved in zagy works in zcli/zdev. COSMOS_CLI_KEYRING is
// left to the other harnesses — this file is the portable store.
package main

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
)

type credFile struct {
	Keys    map[string]string `json:"keys"`
	ChatGPT json.RawMessage   `json:"chatgpt,omitempty"`
}

func credPath() string {
	return filepath.Join(cliHome(), "credentials.json")
}

func loadCreds() credFile {
	f := credFile{Keys: map[string]string{}}
	data, err := os.ReadFile(credPath())
	if err != nil {
		return f
	}
	var raw struct {
		Keys    map[string]string `json:"keys"`
		ChatGPT json.RawMessage   `json:"chatgpt"`
	}
	if json.Unmarshal(data, &raw) != nil {
		return f
	}
	if raw.Keys != nil {
		f.Keys = raw.Keys
	}
	f.ChatGPT = raw.ChatGPT
	return f
}

func saveCreds(f credFile) error {
	if f.Keys == nil {
		f.Keys = map[string]string{}
	}
	p := credPath()
	if err := os.MkdirAll(filepath.Dir(p), 0o755); err != nil {
		return err
	}
	out := map[string]any{"keys": f.Keys}
	if len(f.ChatGPT) > 0 {
		out["chatgpt"] = json.RawMessage(f.ChatGPT)
	}
	b, err := json.MarshalIndent(out, "", "  ")
	if err != nil {
		return err
	}
	tmp := p + ".tmp"
	if err := os.WriteFile(tmp, b, 0o600); err != nil {
		return err
	}
	return os.Rename(tmp, p)
}

func credStored(id string) string {
	if k := strings.TrimSpace(wincredRead(id)); k != "" {
		return k
	}
	return strings.TrimSpace(loadCreds().Keys[id])
}

func credHas(id string) bool {
	return credStored(id) != ""
}

func maskKey(k string) string {
	k = strings.TrimSpace(k)
	if len(k) <= 8 {
		return "••••"
	}
	return k[:4] + "…" + k[len(k)-4:]
}

func storeKey(id, key string) error {
	key = strings.TrimSpace(key)
	if key == "" {
		return errEmptyKey
	}
	if wincredWrite(id, key) {
		f := loadCreds()
		if _, ok := f.Keys[id]; ok {
			delete(f.Keys, id)
			_ = saveCreds(f)
		}
		return nil
	}
	f := loadCreds()
	f.Keys[id] = key
	return saveCreds(f)
}

var errEmptyKey = errStr("empty key")

type errStr string

func (e errStr) Error() string { return string(e) }
