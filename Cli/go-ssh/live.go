// Live model lists — cache in COSMOS_CLI_HOME/live_models.json, shared with zcli/zdev.
package main

import (
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"time"
)

type liveCacheFile map[string]struct {
	At     int64          `json:"at"`
	Models []CatalogModel `json:"models"`
}

func liveCachePath() string { return filepath.Join(cliHome(), "live_models.json") }

func liveLoad() liveCacheFile {
	b, err := os.ReadFile(liveCachePath())
	if err != nil {
		return liveCacheFile{}
	}
	var c liveCacheFile
	if json.Unmarshal(b, &c) != nil {
		return liveCacheFile{}
	}
	return c
}

func liveCached(provider string) []CatalogModel {
	e, ok := liveLoad()[provider]
	if !ok || len(e.Models) == 0 {
		return nil
	}
	return e.Models
}

func liveStore(provider string, models []CatalogModel) {
	c := liveLoad()
	if c == nil {
		c = liveCacheFile{}
	}
	c[provider] = struct {
		At     int64          `json:"at"`
		Models []CatalogModel `json:"models"`
	}{At: time.Now().Unix(), Models: models}
	_ = os.MkdirAll(cliHome(), 0o755)
	b, _ := json.Marshal(c)
	_ = os.WriteFile(liveCachePath(), b, 0o644)
}

func liveFetch(def CatalogProvider) ([]CatalogModel, error) {
	if def.Kind == "devin" {
		return devinModels()
	}
	// HTTP live lists for other kinds are fetched by zcli/zdev; zagy uses
	// the shared cache those CLIs write, plus the catalog snapshot.
	if cached := liveCached(def.ID); len(cached) > 0 {
		return cached, nil
	}
	if len(def.Models) == 0 {
		return nil, fmt.Errorf("%s has no models yet — connect it in zcli/zdev or /providers", def.Name)
	}
	return def.Models, nil
}

func liveGetJSON(url string, headers map[string]string) (map[string]any, error) {
	req, err := http.NewRequest("GET", url, nil)
	if err != nil {
		return nil, err
	}
	for k, v := range headers {
		req.Header.Set(k, v)
	}
	cli := &http.Client{Timeout: 20 * time.Second}
	resp, err := cli.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	b, _ := io.ReadAll(io.LimitReader(resp.Body, 8<<20))
	if resp.StatusCode >= 400 {
		return nil, fmt.Errorf("%s answered %d", catalogHostOf(url), resp.StatusCode)
	}
	var v map[string]any
	if json.Unmarshal(b, &v) != nil {
		return nil, fmt.Errorf("%s sent something that isn't JSON", catalogHostOf(url))
	}
	return v, nil
}

func liveNoise(id string) bool {
	l := strings.ToLower(id)
	if strings.HasSuffix(l, ":batch") {
		return true
	}
	for _, s := range []string{"embed", "tts", "whisper", "dall-e", "moderation", "image", "audio", "speech", "sora"} {
		if strings.Contains(l, s) {
			return true
		}
	}
	return false
}
