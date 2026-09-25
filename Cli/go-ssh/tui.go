// tui.go — bubbletea TUI for zagy (Antigravity-style): transcript viewport,
// `> ` composer with / autocomplete, shortcuts overlay, permission card,
// shift+tab mode cycling, ctrl+r thought expansion.
package main

import (
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"sort"
	"strings"
	"sync"
	"sync/atomic"
	"time"

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
)

type slashCmd struct {
	name string
	desc string
}

var slashCmds = []slashCmd{
	{"/plan", "Switch to plan mode (read-only)"},
	{"/agent", "Switch to agent mode"},
	{"/yolo", "Switch to yolo mode (auto-approve)"},
	{"/approve", "Approve the pending plan"},
	{"/model", "Pick a model"},
	{"/session", "Show session id"},
	{"/usage", "Show token usage"},
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

	busy      bool
	busySince time.Time
	turnSince time.Time
	turnOut   uint64
	totIn     uint64
	totOut    uint64
	cumUSD    float64

	expandThoughts bool

	overlay   overlayKind
	helpTab   int
	helpSel   int
	modelSel  int
	models    []modelRow
	pendModel string

	slashSel    int
	slashOffFor string // slash menu dismissed while this value is in the box

	permIdx   int
	permReply chan int
}

func newTUIModel(a *Agent, opts *RunOpts, cwd string, br *tuiBridge) tuiModel {
	ta := textarea.New()
	ta.Prompt = "> "
	ta.Placeholder = ""
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

func (m tuiModel) Init() tea.Cmd { return textarea.Blink }

func (m tuiModel) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	var cmd tea.Cmd
	switch msg := msg.(type) {
	case tea.WindowSizeMsg:
		m.width, m.height = msg.Width, msg.Height
		m.layout()
		m.reflow()
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
	if m.overlay == overlayHelp {
		return m.helpKey(msg)
	}
	if m.overlay == overlayModels {
		return m.modelKey(msg)
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
	if m.busy {
		m.appendItem(tItem{kind: "note", text: "still working — esc to interrupt"})
		m.reflow()
		return m, nil
	}
	m.appendItem(tItem{kind: "user", text: text})
	m.reflow()
	cmd := m.startTurns(func(emit func(Event)) {
		m.agent.SendUser(text, emit)
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
	case "/plan", "/agent", "/yolo":
		m.setMode(strings.TrimPrefix(cmd, "/"))
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
	case "/session":
		m.appendItem(tItem{kind: "note", text: "session " + a.Session.ID})
	case "/usage":
		m.appendItem(tItem{kind: "note", text: m.usageLine()})
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
	m.agent.Mode = mode
	m.appendItem(tItem{kind: "note", text: "mode: " + mode})
}

func (m *tuiModel) cycleMode() {
	order := []string{"agent", "plan", "yolo"}
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
	u := fmt.Sprintf("tokens · in %d · out %d", m.totIn, m.totOut)
	if m.cumUSD > 0 {
		u += fmt.Sprintf(" · Σ $%.4f", m.cumUSD)
	}
	return u + fmt.Sprintf(" · session %s", m.agent.Session.ID[:8])
}

// ---- model picker ----------------------------------------------------------

func (m *tuiModel) openModels() {
	var aliases []string
	for alias := range m.agent.Cfg.Models {
		aliases = append(aliases, alias)
	}
	sort.Strings(aliases)
	m.models = m.models[:0]
	cur := m.agent.Opts.Model
	for _, alias := range aliases {
		mr := modelRow{alias: alias, entry: m.agent.Cfg.Models[alias],
			current: alias == cur || m.agent.ModelAlias == alias ||
				strings.HasPrefix(m.agent.ModelAlias, alias+" ")}
		m.models = append(m.models, mr)
	}
	m.modelSel = 0
	for i, r := range m.models {
		if r.current {
			m.modelSel = i
		}
	}
	m.overlay = overlayModels
	m.layout()
}

func (m *tuiModel) applyModel(alias string) {
	a := m.agent
	if _, ok := a.Cfg.Models[alias]; !ok {
		m.appendItem(tItem{kind: "note", err: true, text: "no model " + alias})
		return
	}
	if m.busy {
		m.pendModel = alias
		m.appendItem(tItem{kind: "note", text: "model → " + alias + " (applies next turn)"})
		return
	}
	if pcfg, ok := a.Cfg.providerFor(alias); ok {
		a.Provider = pcfg
		a.ModelAlias = alias + " (" + pcfg.Model + ")"
		a.Opts.Model = alias
		m.appendItem(tItem{kind: "note", text: "model: " + a.ModelAlias})
	}
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
		{"shift+tab", "Cycle mode: agent → plan → yolo"},
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
		m.totIn += in + cr + cw
		m.totOut += out
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
		"boot_tokens": a.BootTokens, "resume": optStr(opts.Resume)})
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
