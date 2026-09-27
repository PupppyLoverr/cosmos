// tui.go — bubbletea TUI for zagy (Cosmos family: same galaxy, chrome and
// onboarding as zcli / zdev): transcript viewport, ❯ composer with /
// autocomplete, shortcuts overlay, permission card, shift+tab mode cycling.
package main

import (
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"strings"
	"sync"
	"sync/atomic"
	"time"
	"unicode/utf8"

	"github.com/atotto/clipboard"
	"github.com/charmbracelet/bubbles/spinner"
	"github.com/charmbracelet/bubbles/textarea"
	"github.com/charmbracelet/bubbles/viewport"
	tea "github.com/charmbracelet/bubbletea"
)

const tuiVersion = "0.1.0"

var errInterrupted = errors.New("interrupted")

// messages pumped from the agent goroutine into the program
type eventMsg struct{ ev Event }

type askMsg struct {
	tool  string
	reply chan int // permYes | permAlways | permNo
}

type turnStartMsg struct{}
type turnDoneMsg struct{ err error }

const (
	permYes = iota
	permAlways
	permNo
)

// tuiBridge is shared between the model copies and the agent goroutine.
type tuiBridge struct {
	prog        *tea.Program
	emit        func(Event)
	done        chan struct{}
	interrupted atomic.Bool
	quitOnce    sync.Once
}

// transcript item
type tItem struct {
	kind     string // banner|user|assistant|thought|toolcall|toolresult|plan|perm|note
	text     string
	aux      string // args summary / result preview / draft
	err      bool
	dur      time.Duration // thought
	tok      uint64        // thought output tokens
	resolved int           // perm: -1 pending, else permYes/permAlways/permNo
}

type overlayKind int

const (
	overlayNone overlayKind = iota
	overlayHelp
	overlayModels
	overlayProviders
	overlayKey
	overlayWelcome
)

type slashCmd struct {
	name string
	desc string
}

var slashCmds = []slashCmd{
	{"/plan", "Switch to plan mode (read-only)"},
	{"/agent", "Switch to agent mode"},
	{"/accept-edits", "Writes and edits run without asking"},
	{"/yolo", "Switch to yolo mode (auto-approve)"},
	{"/compact", "Five-layer compact; keep the goal in world.md"},
	{"/undo", "Restore the last file checkpoint (not git)"},
	{"/skillify", "Distill this session into a skill"},
	{"/approve", "Approve the pending plan"},
	{"/model", "Pick a model"},
	{"/providers", "Configure providers — API keys, ChatGPT, Devin subscription"},
	{"/connect", "Add an API key (shared with zcli/zdev)"},
	{"/mode", "Show or switch mode: plan|agent|accept-edits|yolo"},
	{"/context", "Show context window usage"},
	{"/session", "Show session id"},
	{"/usage", "Show token usage"},
	{"/cost", "Show session cost breakdown"},
	{"/init", "Write AGENTS.md for this repository"},
	{"/clear", "Clear the transcript"},
	{"/copy", "Copy the last reply"},
	{"/onboarding", "Replay the first-run setup"},
	{"/help", "Keyboard shortcuts"},
	{"/quit", "Quit zagy"},
}

type modelRow struct {
	alias   string
	entry   ModelEntry
	current bool
}

type tuiModel struct {
	agent  *Agent
	opts   *RunOpts
	cwd    string
	bridge *tuiBridge

	width, height int
	vp            viewport.Model
	ta            textarea.Model
	spin          spinner.Model

	items       []tItem
	lastThought int // index into items, -1 none

	busy          bool
	busySince     time.Time
	turnSince     time.Time
	turnOut       uint64
	totIn         uint64
	totOut        uint64
	totCacheRead  uint64
	totCacheWrite uint64
	turns         uint64
	cumUSD        float64
	modelUSD      map[string]float64
	startedAt     time.Time

	expandThoughts bool

	overlay   overlayKind
	helpTab   int
	helpSel   int
	modelSel  int
	models    []modelRow
	pendModel string

	provSel    int
	provFilter string
	keyProvID  string
	keyBuf     string

	slashSel    int
	slashOffFor string // slash menu dismissed while this value is in the box

	permIdx   int
	permReply chan int

	user    string
	welcome *welcomeFlow
}

func newTUIModel(a *Agent, opts *RunOpts, cwd string, br *tuiBridge) tuiModel {
	ta := textarea.New()
	ta.Prompt = "❯ "
	ta.Placeholder = "Ask zagy — / commands · @ files · ! shell · # remember"
	ta.ShowLineNumbers = false
	ta.EndOfBufferCharacter = ' '
	ta.CharLimit = 0
	ta.MaxHeight = 6
	fs, bs := textarea.DefaultStyles()
	fs.CursorLine = stBase
	fs.Prompt = stPrompt
	fs.EndOfBuffer = stDim
	fs.Placeholder = stDim
	bs = fs
	ta.FocusedStyle = fs
	ta.BlurredStyle = bs
	ta.SetHeight(1)
	ta.SetWidth(80)
	ta.Focus()

	sp := spinner.New()
	sp.Spinner = spinner.MiniDot
	sp.Style = stPrompt

	vp := viewport.New(80, 20)

	m := tuiModel{
		agent: a, opts: opts, cwd: cwd, bridge: br,
		ta: ta, spin: sp, vp: vp,
		width: 80, height: 24,
		lastThought: -1,
		user:        configUserName(),
		modelUSD:    map[string]float64{},
		startedAt:   time.Now(),
	}
	m.items = append(m.items, tItem{kind: "banner"})
	if opts.Resume != "" {
		for _, ev := range a.Session.chain() {
			m.applyEvent(ev, true)
		}
	}
	m.reflow()
	return m
}

func (m tuiModel) Init() tea.Cmd {
	if m.overlay == overlayWelcome && m.welcome != nil {
		return tea.Batch(textarea.Blink, m.welcome.schedule())
	}
	return textarea.Blink
}

func (m *tuiModel) startWelcome() {
	m.welcome = newWelcome(m.width, m.height)
	m.overlay = overlayWelcome
}

func (m *tuiModel) finishWelcome() {
	m.user = configUserName()
	if m.agent != nil {
		m.agent.Cfg = loadConfig(cliHome())
		if d := m.agent.Cfg.DefaultModel; d != "" && d != "fixture" {
			m.applyModel(d)
		}
	}
	m.overlay = overlayNone
	m.welcome = nil
	m.reflow()
}

func (m tuiModel) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	var cmd tea.Cmd
	switch msg := msg.(type) {
	case tea.WindowSizeMsg:
		m.width, m.height = msg.Width, msg.Height
		if m.welcome != nil {
			m.welcome.resize(msg.Width, msg.Height)
		}
		m.layout()
		m.reflow()
		return m, nil
	case welcomeTickMsg:
		if m.overlay == overlayWelcome && m.welcome != nil {
			m.welcome.step(time.Time(msg))
			if m.welcome.quit {
				return m, m.quit()
			}
			if m.welcome.done {
				m.finishWelcome()
				return m, nil
			}
			return m, m.welcome.schedule()
		}
		return m, nil
	case spinner.TickMsg:
		if !m.busy {
			return m, nil
		}
		m.spin, cmd = m.spin.Update(msg)
		return m, cmd
	case turnStartMsg:
		m.turnSince = time.Now()
		return m, nil
	case turnDoneMsg:
		m.busy = false
		if msg.err != nil {
			if errors.Is(msg.err, errInterrupted) {
				m.appendItem(tItem{kind: "note", text: "— interrupted"})
			} else {
				m.appendItem(tItem{kind: "note", text: "✗ " + msg.err.Error(), err: true})
			}
		}
		if m.pendModel != "" {
			a := m.pendModel
			m.pendModel = ""
			m.applyModel(a)
		}
		m.layout()
		m.reflow()
		return m, nil
	case eventMsg:
		m.onEvent(msg.ev)
		return m, nil
	case askMsg:
		m.permIdx = 0
		m.permReply = msg.reply
		m.appendItem(tItem{kind: "perm", text: msg.tool, resolved: -1})
		m.reflow()
		m.vp.GotoBottom()
		return m, nil
	case tea.KeyMsg:
		return m.onKey(msg)
	case tea.MouseMsg:
		m.vp, cmd = m.vp.Update(msg)
		return m, cmd
	}
	return m, nil
}

// ---- key routing -----------------------------------------------------------

func (m tuiModel) onKey(msg tea.KeyMsg) (tea.Model, tea.Cmd) {
	key := msg.String()

	// quit
	if key == "ctrl+c" || (key == "ctrl+d" && m.ta.Value() == "") {
		return m, m.quit()
	}

	// overlays own the keyboard first
	if m.overlay == overlayWelcome && m.welcome != nil {
		cmd := m.welcome.onKey(msg)
		if m.welcome.quit {
			return m, m.quit()
		}
		if m.welcome.done {
			m.finishWelcome()
			return m, nil
		}
		return m, cmd
	}
	if m.overlay == overlayHelp {
		return m.helpKey(msg)
	}
	if m.overlay == overlayModels {
		return m.modelKey(msg)
	}
	if m.overlay == overlayProviders {
		return m.providersKey(msg)
	}
	if m.overlay == overlayKey {
		return m.keyEntryKey(msg)
	}

	switch key {
	case "esc":
		switch {
		case m.permReply != nil:
			m.resolvePerm(permNo)
		case m.slashVisible():
			m.slashOffFor = m.ta.Value()
		case m.busy:
			m.bridge.interrupted.Store(true)
		case m.ta.Value() != "":
			m.ta.Reset()
		}
		return m, nil
	case "ctrl+r":
		m.expandThoughts = !m.expandThoughts
		m.reflow()
		return m, nil
	case "shift+tab":
		if m.permReply == nil {
			m.cycleMode()
		}
		return m, nil
	}

	if m.permReply != nil {
		return m.permKey(msg)
	}

	if m.slashVisible() {
		switch key {
		case "up":
			m.slashSel--
		case "down":
			m.slashSel++
		case "pgup":
			m.slashSel -= 8
		case "pgdown":
			m.slashSel += 8
		case "tab":
			m.clampSlash()
			rows := m.slashRows()
			if len(rows) > 0 {
				m.ta.SetValue("/" + rows[m.slashSel].name[1:])
				m.ta.CursorEnd()
			}
			return m, nil
		case "enter":
			m.clampSlash()
			rows := m.slashRows()
			if len(rows) > 0 {
				m.ta.Reset()
				m.slashOffFor = ""
				return m.execSlash(rows[m.slashSel].name)
			}
			return m, nil
		}
		m.clampSlash()
		if key == "up" || key == "down" || key == "pgup" || key == "pgdown" {
			return m, nil
		}
		// fall through to the textarea so typing refilters the menu
	}

	switch key {
	case "enter":
		// \ + enter = newline fallback (Antigravity convention)
		if v := m.ta.Value(); strings.HasSuffix(v, "\\") {
			m.ta.SetValue(v[:len(v)-1])
			m.ta.CursorEnd()
			m.ta.InsertString("\n")
			m.layout()
			return m, nil
		}
		return m.submit()
	case "alt+enter", "ctrl+j", "shift+enter":
		m.ta.InsertString("\n")
		m.layout()
		return m, nil
	case "?":
		if m.ta.Value() == "" {
			m.overlay = overlayHelp
			m.helpTab = 2
			m.helpSel = 0
			m.layout()
			return m, nil
		}
	case "pgup", "pgdown":
		m.vp, _ = m.vp.Update(msg)
		return m, nil
	}

	var tacmd tea.Cmd
	m.ta, tacmd = m.ta.Update(msg)
	m.clampSlash()
	m.layout()
	return m, tacmd
}

func (m *tuiModel) clampSlash() {
	n := len(m.slashRows())
	if m.slashSel >= n {
		m.slashSel = n - 1
	}
	if m.slashSel < 0 {
		m.slashSel = 0
	}
}

// slashContext reports whether the composer holds a bare `/cmd` prefix.
func (m tuiModel) slashContext() bool {
	v := m.ta.Value()
	return strings.HasPrefix(v, "/") && !strings.ContainsAny(v, " \t\n")
}

func (m tuiModel) slashVisible() bool {
	return m.slashContext() && m.ta.Value() != m.slashOffFor &&
		m.overlay == overlayNone && m.permReply == nil && len(m.slashRows()) > 0
}

// slashRows filters the command table by the composer text.
func (m tuiModel) slashRows() []slashCmd {
	return filterSlash(slashCmds, m.ta.Value())
}

func filterSlash(cmds []slashCmd, typed string) []slashCmd {
	if !strings.HasPrefix(typed, "/") {
		return nil
	}
	var out []slashCmd
	for _, c := range cmds {
		if strings.HasPrefix(c.name, typed) {
			out = append(out, c)
		}
	}
	return out
}

// ---- submitting ------------------------------------------------------------

func (m tuiModel) submit() (tea.Model, tea.Cmd) {
	raw := m.ta.Value()
	m.ta.Reset()
	m.slashOffFor = ""
	m.slashSel = 0
	text := strings.TrimSpace(raw)
	m.layout()
	if text == "" {
		return m, nil
	}
	if strings.HasPrefix(text, "/") {
		return m.execSlash(text)
	}
	if strings.HasPrefix(text, "!") {
		cmd := strings.TrimSpace(strings.TrimPrefix(text, "!"))
		if cmd == "" {
			return m, nil
		}
		out := runTool(m.agent.Ctx, "bash", map[string]any{"command": cmd, "timeout_s": 30})
		m.appendItem(tItem{kind: "note", text: "!" + cmd + "\n" + out})
		m.reflow()
		return m, nil
	}
	if strings.HasPrefix(text, "#") {
		mem := strings.TrimSpace(strings.TrimPrefix(text, "#"))
		if mem == "" {
			return m, nil
		}
		ctx := NewToolCtx(m.cwd)
		ctx.Home = cliHome()
		r := runTool(ctx, "memory_write", map[string]any{"type": "preference", "text": mem, "scope": "zagy"})
		if strings.HasPrefix(r, "ERROR") || strings.Contains(strings.ToLower(r), "error") {
			m.appendItem(tItem{kind: "note", err: true, text: "Memory: " + r})
		} else {
			m.appendItem(tItem{kind: "note", text: "Remembered — “" + shortQuote(mem, 80) + "”"})
		}
		m.reflow()
		return m, nil
	}
	if m.busy {
		m.appendItem(tItem{kind: "note", text: "still working — esc to interrupt"})
		m.reflow()
		return m, nil
	}
	full := expandMentions(text, m.cwd)
	m.appendItem(tItem{kind: "user", text: text})
	m.reflow()
	cmd := m.startTurns(func(emit func(Event)) {
		m.agent.SendUser(full, emit)
	})
	return m, cmd
}

func (m tuiModel) execSlash(text string) (tea.Model, tea.Cmd) {
	fields := strings.Fields(text)
	cmd := fields[0]
	arg := strings.TrimSpace(strings.TrimPrefix(text, cmd))
	a := m.agent
	switch cmd {
	case "/quit", "/exit", "/q":
		return m, m.quit()
	case "/plan", "/agent", "/yolo", "/accept-edits":
		m.setMode(strings.TrimPrefix(cmd, "/"))
	case "/compact":
		focus := strings.TrimSpace(strings.TrimPrefix(strings.TrimPrefix(arg, "focus:"), "focus"))
		m.agent.CompactNow(focus, func(ev Event) {
			if ev.Kind == "compact_thrash" {
				m.appendItem(tItem{kind: "note", err: true, text: "compact thrash — skipped another L5"})
			}
		})
		m.appendItem(tItem{kind: "note", text: "compacted — world.md saved"})
	case "/undo":
		m.appendItem(tItem{kind: "note", text: m.agent.UndoLast(func(Event) {})})
	case "/skillify":
		name := arg
		if name == "" {
			name = "session"
		}
		m.appendItem(tItem{kind: "note", text: m.agent.Skillify(name, func(Event) {})})
	case "/approve":
		if m.busy {
			m.appendItem(tItem{kind: "note", text: "still working — esc to interrupt"})
		} else if a.PendingPlan == "" {
			m.appendItem(tItem{kind: "note", text: "no pending plan"})
		} else {
			m.reflow()
			cmd := m.startTurns(func(emit func(Event)) { a.ApprovePlan(emit) })
			return m, cmd
		}
	case "/model":
		if arg != "" {
			m.applyModel(arg)
		} else {
			m.openModels()
		}
	case "/providers", "/connect":
		if arg != "" {
			m.connectProvider(arg)
		} else {
			m.openProviders()
		}
	case "/mode":
		if arg == "" {
			m.appendItem(tItem{kind: "note", text: "mode: " + a.Mode + " — /mode plan|agent|accept-edits|yolo, or shift+tab"})
		} else if arg == "plan" || arg == "agent" || arg == "accept-edits" || arg == "yolo" {
			m.setMode(arg)
		} else {
			m.appendItem(tItem{kind: "note", err: true, text: "unknown mode " + arg})
		}
	case "/context":
		m.appendItem(tItem{kind: "note", text: a.contextReport()})
	case "/clear":
		m.items = []tItem{{kind: "banner"}}
		m.lastThought = -1
	case "/copy":
		if t := lastAssistantText(m.items); t != "" {
			if err := clipboard.WriteAll(t); err != nil {
				m.appendItem(tItem{kind: "note", err: true, text: "copy failed: " + err.Error()})
			} else {
				m.appendItem(tItem{kind: "note", text: "copied last reply"})
			}
		} else {
			m.appendItem(tItem{kind: "note", text: "nothing to copy"})
		}
	case "/session":
		m.appendItem(tItem{kind: "note", text: "session " + a.Session.ID})
	case "/usage":
		m.appendItem(tItem{kind: "note", text: m.usageLine()})
	case "/cost":
		m.appendItem(tItem{kind: "note", text: m.usageLine()})
	case "/init":
		if msg, err := initProject(m.cwd); err != nil {
			m.appendItem(tItem{kind: "note", err: true, text: "init failed: " + err.Error()})
		} else {
			m.appendItem(tItem{kind: "note", text: msg})
		}
		a.SendUser("/init", renderTUI)
		runTurns(a, renderTUI)
	case "/onboarding":
		m.startWelcome()
		return m, m.welcome.schedule()
	case "/help":
		m.overlay = overlayHelp
		m.helpTab = 0
		m.helpSel = 0
		m.layout()
	case "/keybindings":
		m.overlay = overlayHelp
		m.helpTab = 2
		m.helpSel = 0
		m.layout()
	default:
		m.appendItem(tItem{kind: "note", err: true,
			text: "unknown command " + cmd + " — /help for shortcuts"})
	}
	m.reflow()
	return m, nil
}

func (m *tuiModel) setMode(mode string) {
	if m.busy {
		m.appendItem(tItem{kind: "note",
			text: "busy — switch modes when idle (esc to interrupt)"})
		return
	}
	old := m.agent.Mode
	m.agent.Mode = mode
	m.agent.Session.append("mode", map[string]string{"from": old, "to": mode})
	m.appendItem(tItem{kind: "note", text: "mode: " + mode})
}

func (m *tuiModel) cycleMode() {
	order := []string{"plan", "agent", "accept-edits", "yolo"}
	cur := 0
	for i, v := range order {
		if v == m.agent.Mode {
			cur = i
		}
	}
	m.setMode(order[(cur+1)%len(order)])
	m.reflow()
}

func (m *tuiModel) usageLine() string {
	return formatCostReport(m.totIn, m.totOut, m.totCacheRead, m.totCacheWrite, m.cumUSD, m.modelUSD, m.turns, time.Since(m.startedAt))
}

func formatCostReport(input, output, cacheRead, cacheWrite uint64, total float64, models map[string]float64, turns uint64, wall time.Duration) string {
	out := fmt.Sprintf("Cost\nInput: %d\nOutput: %d\nCache read: %d\nCache write: %d\nUSD total: $%.4f\nTurns: %d\nWall time: %s", input, output, cacheRead, cacheWrite, total, turns, formatWall(wall))
	if len(models) > 1 {
		for model, usd := range models {
			out += fmt.Sprintf("\n%s: $%.4f", model, usd)
		}
	}
	return out
}

func formatWall(d time.Duration) string {
	if d < time.Minute {
		return fmt.Sprintf("%.1fs", d.Seconds())
	}
	return fmt.Sprintf("%.1fm", d.Minutes())
}

// ---- model picker ----------------------------------------------------------

func (m *tuiModel) openModels() {
	m.openModelsFor("")
}

func (m *tuiModel) applyModel(alias string) {
	a := m.agent
	pcfg, ok := a.Cfg.providerFor(alias)
	if !ok {
		m.appendItem(tItem{kind: "note", err: true, text: "no model " + alias + " — try provider/model, e.g. anthropic/claude-opus-5-5"})
		return
	}
	if m.busy {
		m.pendModel = alias
		m.appendItem(tItem{kind: "note", text: "model → " + alias + " (applies next turn)"})
		return
	}
	a.Provider = pcfg
	a.ModelAlias = alias + " (" + pcfg.Model + ")"
	a.Opts.Model = alias
	a.Session.append("model_switch", map[string]string{"alias": alias, "id": pcfg.Model})
	m.appendItem(tItem{kind: "note", text: "model: " + a.ModelAlias})
	_ = setDefaultModel(cliHome(), alias)
}

func (m tuiModel) modelKey(msg tea.KeyMsg) (tea.Model, tea.Cmd) {
	switch msg.String() {
	case "esc":
		m.overlay = overlayNone
		m.layout()
	case "up", "k":
		if m.modelSel > 0 {
			m.modelSel--
		}
	case "down", "j":
		if m.modelSel < len(m.models)-1 {
			m.modelSel++
		}
	case "pgup":
		m.modelSel = 0
	case "pgdown":
		m.modelSel = len(m.models) - 1
	case "enter":
		if m.modelSel >= 0 && m.modelSel < len(m.models) {
			alias := m.models[m.modelSel].alias
			m.overlay = overlayNone
			m.applyModel(alias)
			m.reflow()
		}
		m.layout()
	}
	return m, nil
}

func (m *tuiModel) openProviders() {
	m.provFilter = ""
	m.provSel = 0
	m.overlay = overlayProviders
	m.layout()
}

func (m *tuiModel) connectProvider(id string) {
	p := catalogProvider(id)
	if p == nil {
		m.appendItem(tItem{kind: "note", err: true, text: "unknown provider " + id})
		return
	}
	switch p.Kind {
	case "chatgpt":
		m.overlay = overlayNone
		m.appendItem(tItem{kind: "note", text: "ChatGPT OAuth lives in zcli and zdev — run `zcli` or `zdev`, then /login. Devin and API keys work here."})
		m.layout()
		return
	case "devin":
		m.connectDevin()
		return
	}
	if ready, _ := providerStatus(*p); ready {
		if models, err := liveFetch(*p); err == nil {
			liveStore(p.ID, models)
		}
		m.openModelsFor(p.ID)
		return
	}
	m.keyProvID = p.ID
	m.keyBuf = ""
	m.overlay = overlayKey
	m.layout()
}

func (m *tuiModel) connectDevin() {
	devinInvalidate()
	if !devinReady() {
		m.overlay = overlayNone
		m.appendItem(tItem{kind: "note", err: true, text: devinHint()})
		m.layout()
		return
	}
	def := catalogProvider("devin")
	if def == nil {
		m.appendItem(tItem{kind: "note", err: true, text: "Devin is missing from the catalog"})
		return
	}
	models, err := liveFetch(*def)
	if err != nil {
		m.overlay = overlayNone
		m.appendItem(tItem{kind: "note", err: true, text: err.Error()})
		m.layout()
		return
	}
	liveStore("devin", models)
	m.agent.Cfg = loadConfig(cliHome())
	m.appendItem(tItem{kind: "note", text: fmt.Sprintf("Devin · %s · %d models", devinDetail(), len(models))})
	m.openModelsFor("devin")
}

func (m *tuiModel) openModelsFor(provider string) {
	cur := m.agent.Opts.Model
	choices := m.agent.Cfg.availableModels(cur)
	m.models = m.models[:0]
	m.modelSel = 0
	for _, c := range choices {
		if provider != "" && c.provider != provider {
			continue
		}
		mr := modelRow{
			alias: c.alias,
			entry: ModelEntry{Provider: c.provider, ID: c.id, Context: float64(c.context)},
			current: c.current || c.alias == m.agent.Opts.Model ||
				strings.HasPrefix(m.agent.ModelAlias, c.alias),
		}
		if mr.current {
			m.modelSel = len(m.models)
		}
		m.models = append(m.models, mr)
	}
	m.overlay = overlayModels
	m.layout()
}

func (m *tuiModel) saveProviderKey() {
	id := m.keyProvID
	key := strings.TrimSpace(m.keyBuf)
	m.keyBuf = ""
	if id == "" || key == "" {
		m.overlay = overlayProviders
		m.layout()
		return
	}
	p := catalogProvider(id)
	if p == nil {
		m.overlay = overlayProviders
		m.layout()
		return
	}
	if err := storeKey(id, key); err != nil {
		m.overlay = overlayNone
		m.appendItem(tItem{kind: "note", err: true, text: p.Name + ": " + err.Error()})
		m.layout()
		return
	}
	wp := welcomeProv{id: p.ID, name: p.Name, env: "", kind: p.Kind, base: p.BaseURL}
	if len(p.Env) > 0 {
		wp.env = p.Env[0]
	}
	_ = ensureProvider(cliHome(), wp)
	m.agent.Cfg = loadConfig(cliHome())
	if models, err := liveFetch(*p); err == nil {
		liveStore(p.ID, models)
	}
	m.keyProvID = ""
	m.appendItem(tItem{kind: "note", text: "✓ " + catalogDisplayName(*p) + " key saved — works in zcli and zdev too"})
	m.openModelsFor(p.ID)
}

func (m tuiModel) providersKey(msg tea.KeyMsg) (tea.Model, tea.Cmd) {
	rows := filterProviders(m.provFilter)
	n := len(rows)
	if m.provSel >= n && n > 0 {
		m.provSel = n - 1
	}
	switch msg.String() {
	case "esc":
		m.overlay = overlayNone
		m.layout()
	case "up", "k":
		if m.provSel > 0 {
			m.provSel--
		}
	case "down", "j":
		if m.provSel < n-1 {
			m.provSel++
		}
	case "pgup":
		m.provSel = 0
	case "pgdown":
		if n > 0 {
			m.provSel = n - 1
		}
	case "enter":
		if m.provSel >= 0 && m.provSel < n {
			m.connectProvider(rows[m.provSel].ID)
		}
	case "backspace":
		if m.provFilter != "" {
			_, sz := utf8.DecodeLastRuneInString(m.provFilter)
			m.provFilter = m.provFilter[:len(m.provFilter)-sz]
			m.provSel = 0
		}
	default:
		if msg.Type == tea.KeyRunes {
			m.provFilter += string(msg.Runes)
			m.provSel = 0
		}
	}
	return m, nil
}

func (m tuiModel) keyEntryKey(msg tea.KeyMsg) (tea.Model, tea.Cmd) {
	switch msg.String() {
	case "esc":
		m.keyBuf = ""
		m.keyProvID = ""
		m.overlay = overlayProviders
		m.layout()
	case "enter":
		m.saveProviderKey()
	case "backspace":
		if m.keyBuf != "" {
			_, sz := utf8.DecodeLastRuneInString(m.keyBuf)
			m.keyBuf = m.keyBuf[:len(m.keyBuf)-sz]
		}
	default:
		if msg.Type == tea.KeyRunes {
			m.keyBuf += string(msg.Runes)
		}
	}
	return m, nil
}

// ---- help overlay ----------------------------------------------------------

func (m tuiModel) helpKey(msg tea.KeyMsg) (tea.Model, tea.Cmd) {
	n := len(m.helpRows())
	switch msg.String() {
	case "esc":
		m.overlay = overlayNone
		m.layout()
	case "tab", ">", "l":
		m.helpTab = (m.helpTab + 1) % 3
		m.helpSel = 0
	case "<", "h", "shift+tab":
		m.helpTab = (m.helpTab + 2) % 3
		m.helpSel = 0
	case "up", "k":
		if m.helpSel > 0 {
			m.helpSel--
		}
	case "down", "j":
		if m.helpSel < n-1 {
			m.helpSel++
		}
	case "pgup":
		m.helpSel -= m.helpPage()
	case "pgdown":
		m.helpSel += m.helpPage()
	}
	if m.helpSel < 0 {
		m.helpSel = 0
	}
	if m.helpSel > n-1 {
		m.helpSel = n - 1
	}
	return m, nil
}

type helpRow struct{ left, right string }

func (m tuiModel) helpRows() []helpRow {
	a := m.agent
	switch m.helpTab {
	case 0: // general
		allow := strings.Join(a.Opts.Allow, ", ")
		if allow == "" {
			allow = "—"
		}
		sandbox := "appcontainer"
		if a.Ctx.NoSandbox {
			sandbox = "off"
		}
		if a.Ctx.SandboxDegraded != "" {
			sandbox = "degraded: " + a.Ctx.SandboxDegraded
		}
		return []helpRow{
			{"model", a.ModelAlias},
			{"mode", a.Mode},
			{"session", a.Session.ID},
			{"workspace", a.Workspace},
			{"sandbox", sandbox},
			{"boot tokens", fmt.Sprintf("%d", a.BootTokens)},
			{"max turns", fmt.Sprintf("%d", a.Opts.MaxTurns)},
			{"allow", allow},
			{"tokens", fmt.Sprintf("in %d · out %d", m.totIn, m.totOut)},
			{"cost", fmt.Sprintf("$%.4f", m.cumUSD)},
		}
	case 1: // commands
		var rows []helpRow
		for _, c := range slashCmds {
			rows = append(rows, helpRow{c.name, c.desc})
		}
		rows = append(rows, helpRow{"/keybindings", "Open the shortcuts tab"})
		return rows
	default: // shortcuts
		return shortcutRows()
	}
}

func shortcutRows() []helpRow {
	return []helpRow{
		{"/", "Open slash commands"},
		{"\\ + enter", "Insert newline fallback"},
		{"alt+enter, ctrl+j, shift+enter", "Insert newline"},
		{"?", "Open shortcuts (empty input)"},
		{"shift+tab", "Cycle mode: plan → agent → accept-edits → yolo"},
		{"ctrl+r", "Toggle thought / draft view"},
		{"esc", "Close overlay · interrupt · clear input"},
		{"ctrl+c, ctrl+d", "Exit"},
		{"pgup, pgdown", "Scroll transcript"},
		{"ctrl+u", "Delete line before cursor"},
		{"up, down", "Navigate menus"},
		{"tab", "Complete slash command"},
		{"enter", "Send message or confirm"},
		{"y, a, n", "Answer permission prompt"},
	}
}

// ---- permission card -------------------------------------------------------

func (m tuiModel) permKey(msg tea.KeyMsg) (tea.Model, tea.Cmd) {
	switch msg.String() {
	case "y", "Y":
		m.resolvePerm(permYes)
	case "a", "A":
		m.resolvePerm(permAlways)
	case "n", "N":
		m.resolvePerm(permNo)
	case "up", "left", "k", "h":
		m.permIdx = (m.permIdx + 2) % 3
	case "down", "right", "j", "l", "tab":
		m.permIdx = (m.permIdx + 1) % 3
	case "enter", " ":
		m.resolvePerm(m.permIdx)
	case "pgup", "pgdown":
		m.vp, _ = m.vp.Update(msg)
	}
	return m, nil
}

func (m *tuiModel) resolvePerm(choice int) {
	if m.permReply == nil {
		return
	}
	select {
	case m.permReply <- choice:
	default:
	}
	m.permReply = nil
	// flip the live card into a static outcome line
	for i := len(m.items) - 1; i >= 0; i-- {
		if m.items[i].kind == "perm" && m.items[i].resolved == -1 {
			m.items[i].resolved = choice
			break
		}
	}
	m.reflow()
}

// ---- events ----------------------------------------------------------------

func (m *tuiModel) onEvent(ev Event) {
	m.applyEvent(ev, false)
	m.reflow()
}

// applyEvent maps a session event onto transcript items. history=true is used
// when replaying a resumed session (user lines aren't skipped, no thought row).
func (m *tuiModel) applyEvent(ev Event, history bool) {
	switch ev.Kind {
	case "user":
		if history {
			m.items = append(m.items, tItem{kind: "user", text: dataStr(ev, "text")})
		}
		// live user events are echoed locally on submit — skip
	case "assistant":
		text := dataStr(ev, "text")
		if !history {
			m.items = append(m.items, tItem{
				kind: "thought", dur: time.Since(m.turnSince), aux: text})
			m.lastThought = len(m.items) - 1
		}
		if strings.TrimSpace(text) != "" {
			m.items = append(m.items, tItem{kind: "assistant", text: text})
		}
		for _, c := range anySlice(dataGet(ev, "tool_calls")) {
			cm, _ := c.(map[string]any)
			m.items = append(m.items, tItem{kind: "toolcall",
				text: strv(cm, "name"), aux: shortArgs(cm["input"])})
		}
	case "tool_result":
		isErr, _ := dataGet(ev, "is_error").(bool)
		m.items = append(m.items, tItem{kind: "toolresult",
			text: dataStr(ev, "name"), aux: dataStr(ev, "content"), err: isErr})
	case "plan":
		m.items = append(m.items, tItem{kind: "plan", text: dataStr(ev, "plan")})
	case "usage":
		in := uint64(numv(dataGetMap(ev), "input"))
		cr := uint64(numv(dataGetMap(ev), "cache_read"))
		cw := uint64(numv(dataGetMap(ev), "cache_write"))
		out := uint64(numv(dataGetMap(ev), "output"))
		m.totIn += in
		m.totCacheRead += cr
		m.totCacheWrite += cw
		m.totOut += out
		m.turns++
		if model := dataStr(ev, "model"); model != "" {
			if usd, ok := dataGet(ev, "turn_usd").(float64); ok {
				m.modelUSD[model] += usd
			}
		}
		m.turnOut += out
		if c, ok := dataGet(ev, "cum_usd").(float64); ok {
			m.cumUSD = c
		}
		if m.lastThought >= 0 && m.lastThought < len(m.items) &&
			m.items[m.lastThought].kind == "thought" {
			m.items[m.lastThought].tok = out
		}
	case "mode":
		m.items = append(m.items, tItem{kind: "note",
			text: "mode: " + dataStr(ev, "from") + " → " + dataStr(ev, "to")})
	case "compaction":
		m.items = append(m.items, tItem{kind: "note",
			text: fmt.Sprintf("◇ compacted context · %d → %d tokens",
				int64(numv(dataGetMap(ev), "before_tokens")),
				int64(numv(dataGetMap(ev), "after_tokens")))})
	case "subagent_start":
		m.items = append(m.items, tItem{kind: "note",
			text: "⎇ subagent: " + dataStr(ev, "prompt")})
	case "permission_decision":
		word := "denied"
		if g, _ := dataGet(ev, "granted").(bool); g {
			word = "granted"
		}
		if cap := dataStr(ev, "capability"); cap != "" {
			m.items = append(m.items, tItem{kind: "note",
				text: "escalation " + cap + " " + word})
		} else if history {
			m.items = append(m.items, tItem{kind: "note",
				text: "▌ " + dataStr(ev, "tool") + " — " + word})
		}
		// live tool decisions are rendered by the perm card itself
	case "session_end":
		m.items = append(m.items, tItem{kind: "note",
			text: "■ session ended (" + dataStr(ev, "reason") + ")"})
	}
}

func dataGetMap(e Event) map[string]any {
	var out map[string]any
	json.Unmarshal(e.Data, &out)
	return out
}

// ---- turn driver (runs in a tea.Cmd goroutine) -----------------------------

func (m *tuiModel) startTurns(pre func(emit func(Event))) tea.Cmd {
	m.busy = true
	m.busySince = time.Now()
	m.turnSince = m.busySince
	m.turnOut = 0
	m.bridge.interrupted.Store(false)
	m.layout()
	a := m.agent
	bridge := m.bridge
	return tea.Batch(m.spin.Tick, func() tea.Msg {
		emit := bridge.emit
		if pre != nil {
			pre(emit)
		}
		max := a.Opts.MaxTurns
		if max <= 0 {
			max = 32
		}
		for i := 0; i < max; i++ {
			if bridge.interrupted.Load() {
				return turnDoneMsg{errInterrupted}
			}
			bridge.prog.Send(turnStartMsg{})
			done, err := a.Turn(emit)
			if err != nil {
				return turnDoneMsg{err}
			}
			if done || bridge.interrupted.Load() {
				if bridge.interrupted.Load() {
					return turnDoneMsg{errInterrupted}
				}
				return turnDoneMsg{}
			}
			last := a.Messages[len(a.Messages)-1]
			var calls []map[string]any
			for _, b := range anySlice(last["content"]) {
				if bm, _ := b.(map[string]any); bm["type"] == "tool_use" {
					calls = append(calls, bm)
				}
			}
			for i, bm := range calls {
				if bridge.interrupted.Load() {
					// providers require a tool_result per tool_use — stub the rest
					for _, rb := range calls[i:] {
						stubToolResult(a, emit, rb)
					}
					return turnDoneMsg{errInterrupted}
				}
				inp, _ := bm["input"].(map[string]any)
				a.ExecTool(strv(bm, "id"), strv(bm, "name"), inp, emit)
				if strv(bm, "name") == "submit_plan" {
					return turnDoneMsg{}
				}
			}
		}
		return turnDoneMsg{}
	})
}

// stubToolResult appends a denied tool_result for an unexecuted tool_use so
// the message log stays well-formed after an interrupt.
func stubToolResult(a *Agent, emit func(Event), bm map[string]any) {
	id, name := strv(bm, "id"), strv(bm, "name")
	text := "interrupted by operator"
	emit(a.Session.append("tool_result", map[string]any{
		"tool_use_id": id, "name": name, "content": text, "is_error": true}))
	a.Messages = append(a.Messages, map[string]any{
		"role": "user",
		"content": []any{map[string]any{
			"type": "tool_result", "tool_use_id": id,
			"content": text, "is_error": true}},
	})
}

// ---- plumbing ---------------------------------------------------------------

func (m *tuiModel) appendItem(it tItem) {
	m.items = append(m.items, it)
	if len(m.items) > 200 {
		m.items = m.items[len(m.items)-200:]
	}
}

func (m *tuiModel) reflow() {
	stick := m.vp.AtBottom()
	m.vp.SetContent(strings.Join(m.renderItems(m.vp.Width), "\n"))
	if stick {
		m.vp.GotoBottom()
	}
}

func (m *tuiModel) layout() {
	// composer height = wrapped visual line count, capped at 6
	textW := m.width - 2 // prompt "> "
	if textW < 8 {
		textW = 8
	}
	h := 0
	for _, l := range strings.Split(m.ta.Value(), "\n") {
		n := len([]rune(l))
		h += clampInt((n+textW-1)/textW, 1, 6)
	}
	h = clampInt(h, 1, 6)
	m.ta.SetHeight(h)
	chrome := h + 2 + 1 // composer + rules + footer
	if m.busy {
		chrome++
	}
	if m.slashVisible() {
		chrome += m.slashHeight()
	}
	if m.overlay != overlayNone {
		chrome += m.overlayHeight()
	}
	vh := m.height - chrome
	if vh < 3 {
		vh = 3
	}
	m.vp.Height = vh
	m.vp.Width = m.width
	m.ta.SetWidth(m.width)
}

func (m tuiModel) slashHeight() int {
	n := len(m.slashRows())
	if n > 8 {
		n = 8
	}
	return n + 1 // + overflow indicator line
}

func (m tuiModel) helpPage() int {
	p := m.height - 14
	if p < 4 {
		p = 4
	}
	if p > 24 {
		p = 24
	}
	return p
}

func (m tuiModel) overlayHeight() int {
	switch m.overlay {
	case overlayHelp:
		n := len(m.helpRows())
		if n > m.helpPage() {
			n = m.helpPage()
		}
		return 3 + n + 1 // tabbar+title+note + rows + counter
	case overlayModels:
		return 1 + len(m.models) // title + rows
	case overlayProviders:
		n := len(filterProviders(m.provFilter))
		if n > 12 {
			n = 12
		}
		return 4 + n + 2
	case overlayKey:
		return 8
	}
	return 0
}

func (m tuiModel) quit() tea.Cmd {
	m.bridge.quitOnce.Do(func() {
		close(m.bridge.done)
		if m.permReply != nil {
			select {
			case m.permReply <- permNo:
			default:
			}
		}
		m.agent.Session.append("session_end", map[string]string{"reason": "quit"})
		m.agent.cleanup()
	})
	return tea.Quit
}

// ---- entry -----------------------------------------------------------------

func stdinTTY() bool {
	fi, err := os.Stdin.Stat()
	if err != nil {
		return false
	}
	return fi.Mode()&os.ModeCharDevice != 0
}

func runTUI(opts *RunOpts, cwd string) int {
	if !stdinTTY() {
		return repl(opts)
	}
	a, err := NewAgent(opts, "zagy", cwd)
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		return ExitFailed
	}
	bridge := &tuiBridge{done: make(chan struct{})}
	m := newTUIModel(a, opts, cwd, bridge)
	if welcomeNeeded() {
		m.startWelcome()
	}
	p := tea.NewProgram(m)
	bridge.prog = p
	bridge.emit = func(ev Event) { p.Send(eventMsg{ev}) }
	a.Asker = func(tool string) bool {
		ch := make(chan int, 1)
		p.Send(askMsg{tool: tool, reply: ch})
		select {
		case r := <-ch:
			if r == permAlways {
				a.Opts.Allow = append(a.Opts.Allow, tool)
			}
			return r != permNo
		case <-bridge.done:
			return false
		}
	}
	a.LogW = io.Discard // keep per-turn stats out of the TUI frame
	defer a.cleanup()
	a.Session.append("session_start", map[string]any{
		"model": a.ModelAlias, "mode": a.Mode,
		"sandbox":     map[bool]string{true: "off", false: "appcontainer"}[a.Ctx.NoSandbox],
		"boot_tokens": a.BootTokens, "resume": optStr(opts.Resume),
		"cwd": a.Workspace})
	runHooks(a.Cfg, "session_start", "", map[string]any{}, a.Session.ID)
	fm, err := p.Run()
	if fm2, ok := fm.(tuiModel); ok {
		fm2.quit() // ensure session_end on abrupt exits (quitOnce-guarded)
	}
	if err != nil {
		fmt.Fprintf(os.Stderr, "tui: %v — falling back to line mode\n", err)
		return lineREPL(a, opts)
	}
	return ExitDone
}
