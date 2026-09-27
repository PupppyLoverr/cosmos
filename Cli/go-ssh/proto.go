// zproto — wire types shared by every Cosmos CLI harness (CONTRACT.md §6).
package main

import (
	"crypto/rand"
	"encoding/binary"
	"encoding/json"
	"math/big"
	"os"
	"time"
)

const ContractVersion = 1

// exit codes (contract §7)
const (
	ExitDone     = 0
	ExitFailed   = 1
	ExitUsage    = 2
	ExitDenied   = 3
	ExitProvider = 4
	ExitBudget   = 5
)

var crockford = []byte("0123456789abcdefghjkmnpqrstvwxyz")

// newID — ULID: 48-bit ms timestamp in the high bits + 80-bit randomness,
// Crockford base32, 26 chars.
func newID() string {
	b := make([]byte, 16)
	binary.BigEndian.PutUint64(b[:8], uint64(time.Now().UnixMilli())<<16)
	rand.Read(b[8:])
	n := new(big.Int).SetBytes(b)
	m := new(big.Int)
	base := big.NewInt(32)
	var out [26]byte
	for i := 25; i >= 0; i-- {
		n.DivMod(n, base, m)
		out[i] = crockford[m.Int64()]
	}
	return string(out[:])
}

func nowTS() string {
	return time.Now().UTC().Format("2006-01-02T15:04:05.000Z")
}

type Event struct {
	V       int             `json:"v"`
	ID      string          `json:"id"`
	Parent  *string         `json:"parent"`
	TS      string          `json:"ts"`
	Session string          `json:"session"`
	Harness string          `json:"harness"`
	Kind    string          `json:"kind"`
	Data    json.RawMessage `json:"data"`
}

func newEvent(session, harness string, parent *string, kind string, data any) Event {
	raw, _ := json.Marshal(data)
	return Event{
		V: ContractVersion, ID: newID(), Parent: parent, TS: nowTS(),
		Session: session, Harness: harness, Kind: kind, Data: raw,
	}
}

func (e Event) Line() string {
	b, _ := json.Marshal(e)
	return string(b)
}

type RunOpts struct {
	Prompt             *string
	Output             string // text|json|json-stream
	Mode               string // ""|plan|agent|accept-edits
	Allow              []string
	Yolo               bool
	NoSandbox          bool
	Model              string
	MaxTurns           int
	MaxUSD             float64
	Cwd                string
	Resume             string
	AddDirs            []string
	SystemPrompt       string
	AppendSystemPrompt string
	ApprovePlan        bool
	AllowEscalation    []string
	Sandbox            string
	Ask                string
	Autonomous         bool
	AutonomousGate     string
	AllowSelfMutate    bool
	Arena              bool
	SelfImprove        *bool
}

func cliHome() string {
	if v := os.Getenv("COSMOS_CLI_HOME"); v != "" {
		return v
	}
	base := os.Getenv("HOME")
	if base == "" {
		base = os.Getenv("USERPROFILE")
	}
	if base == "" {
		base, _ = os.UserHomeDir()
	}
	return base + string(os.PathSeparator) + ".cosmos-cli"
}

func strv(m map[string]any, k string) string {
	if v, ok := m[k].(string); ok {
		return v
	}
	return ""
}

func numv(m map[string]any, k string) float64 {
	switch v := m[k].(type) {
	case float64:
		return v
	case int64:
		return float64(v)
	}
	return 0
}

func dataGet(e Event, key string) any {
	var m map[string]any
	if json.Unmarshal(e.Data, &m) == nil {
		return m[key]
	}
	return nil
}

func dataStr(e Event, key string) string {
	if s, ok := dataGet(e, key).(string); ok {
		return s
	}
	return ""
}
