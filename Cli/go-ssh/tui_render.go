// tui_render.go — zagy TUI rendering: banner, transcript items, mini-markdown,
// slash dropdown, help/model overlays, permission card, footer.
package main

import (
	"encoding/json"
	"fmt"
	"sort"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/charmbracelet/lipgloss"
	"github.com/charmbracelet/x/ansi"
)

// palette — Cosmos family (zcli theme.rs / zdev welcome): near-white body,
// soft grey secondary, darker rules, one cyan accent, galaxy shades.
var (
	stBase   = lipgloss.NewStyle()
	stPrompt = lipgloss.NewStyle().Foreground(lipgloss.Color("#5cd0ff")).Bold(true)
	stSel    = lipgloss.NewStyle().Foreground(lipgloss.Color("#5cd0ff"))
	stSelB   = lipgloss.NewStyle().Foreground(lipgloss.Color("#5cd0ff")).Bold(true)
	stDim    = lipgloss.NewStyle().Foreground(lipgloss.Color("#8a8a8a"))
	stDimmer = lipgloss.NewStyle().Foreground(lipgloss.Color("#4a4a4a"))
	stGhost  = lipgloss.NewStyle().Foreground(lipgloss.Color("#6c6c6c"))
	stBold   = lipgloss.NewStyle().Bold(true)
	stErr    = lipgloss.NewStyle().Foreground(lipgloss.Color("#ff6b6b"))
	stWarn   = lipgloss.NewStyle().Foreground(lipgloss.Color("#e5d892"))
	stGreen  = lipgloss.NewStyle().Foreground(lipgloss.Color("#5fd787"))
	stCode   = lipgloss.NewStyle().Foreground(lipgloss.Color("#e6b473"))
	stKey    = lipgloss.NewStyle().Foreground(lipgloss.Color("#5cd0ff")).Bold(true)
	stRose   = lipgloss.NewStyle().Foreground(lipgloss.Color("#e8e8e8"))
	stWhite  = lipgloss.NewStyle().Foreground(lipgloss.Color("#f4f4f4")).Bold(true)
)

func rule(w int) string {
	if w < 1 {
		w = 1
	}
	return stDimmer.Render(strings.Repeat("─", w))
}

func logoRowStyled(row []logoCell) string {
	var b strings.Builder
	for _, c := range row {
		sh := logoShade(c.lum)
		st := lipgloss.NewStyle().Foreground(lipgloss.Color(sh.color))
		if sh.bold {
			st = st.Bold(true)
		}
		b.WriteString(st.Render(string(c.ch)))
	}
	return b.String()
}

func tilde(p string) string {
	home := cliHome()
	// cliHome ends with .cosmos-cli; the real home is its parent
	if i := strings.LastIndex(home, ".cosmos-cli"); i > 0 {
		home = home[:i]
		home = strings.TrimRight(home, `/\`)
	}
	if home != "" && strings.HasPrefix(strings.ToLower(p), strings.ToLower(home)) {
		return "~" + p[len(home):]
	}
	return p
}

func clampInt(v, lo, hi int) int {
	if v < lo {
		return lo
	}
	if v > hi {
		return hi
	}
	return v
}

func truncANSI(s string, w int) string {
	if w < 4 {
		w = 4
	}
	return ansi.Truncate(s, w, "…")
}

func humanTok(n uint64) string {
	trim := func(s string) string {
		return strings.TrimSuffix(s, ".0")
	}
	switch {
	case n >= 1_000_000:
		return trim(fmt.Sprintf("%.1f", float64(n)/1e6)) + "M"
	case n >= 1000:
		return trim(fmt.Sprintf("%.1f", float64(n)/1e3)) + "k"
	default:
		return fmt.Sprintf("%d", n)
	}
}

// ---- view ------------------------------------------------------------------

func (m tuiModel) View() string {
	if m.width == 0 {
		return ""
	}
	if m.overlay == overlayWelcome && m.welcome != nil {
		return m.welcome.View()
	}
	var b strings.Builder
	b.WriteString(m.vp.View())
	b.WriteByte('\n')
	if m.busy {
		b.WriteString(m.busyLine())
		b.WriteByte('\n')
	}
	b.WriteString(rule(m.width))
	b.WriteByte('\n')
	b.WriteString(m.ta.View())
	b.WriteByte('\n')
	b.WriteString(rule(m.width))
	b.WriteByte('\n')
	if m.slashVisible() {
		b.WriteString(m.slashMenu())
		b.WriteByte('\n')
	}
	switch m.overlay {
	case overlayHelp:
		b.WriteString(m.helpView())
		b.WriteByte('\n')
	case overlayModels:
		b.WriteString(m.modelView())
		b.WriteByte('\n')
	case overlayProviders:
		b.WriteString(m.providersView())
		b.WriteByte('\n')
	case overlayKey:
		b.WriteString(m.keyEntryView())
		b.WriteByte('\n')
	}
	b.WriteString(m.footer())
	return b.String()
}

func (m tuiModel) busyLine() string {
	secs := time.Since(m.busySince).Seconds()
	what := "Thinking…"
	if m.permReply != nil {
		what = "Waiting for approval…"
	}
	return m.spin.View() + " " + stDim.Render(fmt.Sprintf(
		"%s %s · ↓%s tokens", what, fmtDur(secs), humanTok(m.turnOut))) +
		stDimmer.Render(" · esc to interrupt")
}

func fmtDur(secs float64) string {
	if secs < 1 {
		return fmt.Sprintf("%.1fs", secs)
	}
	return fmt.Sprintf("%.0fs", secs)
}

func (m tuiModel) footer() string {
	switch {
	case m.permReply != nil:
		return stKey.Render("y") + stDim.Render(" Yes   ") +
			stKey.Render("a") + stDim.Render(" Yes, always   ") +
			stKey.Render("n") + stDim.Render(" No   ") +
			stDimmer.Render("·   ↑/↓ select · enter confirm")
	case m.overlay == overlayHelp:
		return stDim.Render("Keyboard: ") + stKey.Render("↑/↓") + stDim.Render(" Navigate   ") +
			stKey.Render("</>") + stDim.Render(" Switch View   ") +
			stKey.Render("esc") + stDim.Render(" Close")
	case m.overlay == overlayModels:
		return stKey.Render("↑/↓") + stDim.Render(" Navigate   ") +
			stKey.Render("enter") + stDim.Render(" Select   ") +
			stKey.Render("esc") + stDim.Render(" Cancel")
	case m.overlay == overlayProviders:
		return stKey.Render("↑/↓") + stDim.Render(" Navigate   ") +
			stKey.Render("enter") + stDim.Render(" Connect   ") +
			stKey.Render("esc") + stDim.Render(" Cancel")
	case m.overlay == overlayKey:
		return stKey.Render("enter") + stDim.Render(" Save   ") +
			stKey.Render("esc") + stDim.Render(" Back")
	case m.slashVisible():
		return stKey.Render("↑/↓") + stDim.Render(" Navigate · ") +
			stKey.Render("enter") + stDim.Render(" Select · ") +
			stKey.Render("tab") + stDim.Render(" Complete · ") +
			stKey.Render("esc") + stDim.Render(" to cancel")
	default:
		left := stDim.Render("? for shortcuts")
		right := stDim.Render(m.modelLabel() + " · " + m.agent.Mode)
		if m.totIn+m.totOut > 0 {
			ctx := ""
			if r := m.agent.contextReport(); r != "" {
				var mm map[string]any
				if json.Unmarshal([]byte(r), &mm) == nil {
					ctx = fmt.Sprintf(" · ctx %.0f%%", numv(mm, "percent"))
				}
			}
			right = stDim.Render(m.modelLabel()+" · "+m.agent.Mode+" · ") +
				stDimmer.Render(fmt.Sprintf("↑%s ↓%s%s", humanTok(m.totIn), humanTok(m.totOut), ctx))
		}
		pad := m.width - lipgloss.Width(left) - lipgloss.Width(right)
		if pad < 1 {
			pad = 1
		}
		return left + strings.Repeat(" ", pad) + right
	}
}

func (m tuiModel) modelLabel() string {
	lab := m.agent.ModelAlias
	if i := strings.Index(lab, " ("); i > 0 {
		lab = lab[:i]
	}
	return lab
}

// ---- transcript items ------------------------------------------------------

func (m tuiModel) renderItems(w int) []string {
	if w < 8 {
		w = 8
	}
	var out []string
	for _, it := range m.items {
		out = append(out, m.renderItem(it, w)...)
	}
	return out
}

func (m tuiModel) renderItem(it tItem, w int) []string {
	switch it.kind {
	case "banner":
		return m.renderBanner(w)
	case "user":
		return renderUser(it.text, w)
	case "assistant":
		return renderMarkdown(it.text, w)
	case "thought":
		return m.renderThought(it, w)
	case "toolcall":
		head := stSel.Render("●") + " " + stBold.Render(it.text)
		if it.aux != "" {
			head += " " + stDim.Render(truncANSI(it.aux, w-lipgloss.Width(head)-1))
		}
		return []string{head}
	case "toolresult":
		return renderToolResult(it, w)
	case "plan":
		return renderPlan(it.text, w)
	case "perm":
		return m.renderPerm(it, w)
	default: // note
		st := stDim
		if it.err {
			st = stErr
		}
		return wrapStyled(st.Render(it.text), w, "  ")
	}
}

func renderUser(text string, w int) []string {
	var out []string
	for i, line := range strings.Split(text, "\n") {
		wrapped := ansi.Wordwrap(line, w-2, "")
		for j, wl := range strings.Split(wrapped, "\n") {
			if i == 0 && j == 0 {
				out = append(out, stPrompt.Render(">")+" "+wl)
			} else {
				out = append(out, "  "+wl)
			}
		}
	}
	return out
}

func (m tuiModel) renderThought(it tItem, w int) []string {
	head := "▶ "
	if m.expandThoughts {
		head = "▼ "
	}
	line := stDim.Render(head + "Thought for " + fmtDur(it.dur.Seconds()))
	if it.tok > 0 {
		line = stDim.Render(head + "Thought for " + fmtDur(it.dur.Seconds()) +
			fmt.Sprintf(", %d tokens", it.tok))
	}
	if !m.expandThoughts {
		return []string{line}
	}
	out := []string{line}
	draft := strings.TrimSpace(it.aux)
	if draft == "" {
		out = append(out, stDimmer.Render("  (no draft recorded)"))
		return out
	}
	for _, l := range renderMarkdown(draft, w-2) {
		// renderMarkdown returns styled text; dim it wholesale
		out = append(out, "  "+stDimmer.Render(ansi.Strip(l)))
	}
	return out
}

func renderToolResult(it tItem, w int) []string {
	mark := stDim.Render("⎿")
	if it.err {
		mark = stErr.Render("✗")
	}
	st := stDim
	if it.err {
		st = stErr
	}
	var lines []string
	for _, l := range strings.Split(it.aux, "\n") {
		if strings.TrimSpace(l) != "" {
			lines = append(lines, l)
		}
		if len(lines) == 2 {
			break
		}
	}
	if len(lines) == 0 {
		return []string{"  " + mark + " " + st.Render("(empty)")}
	}
	var out []string
	for i, l := range lines {
		l = strings.TrimSpace(l)
		if len(l) > 140 {
			l = l[:140] + "…"
		}
		if i == 0 {
			out = append(out, "  "+mark+" "+st.Render(truncANSI(l, w-5)))
		} else {
			out = append(out, "    "+st.Render(truncANSI(l, w-5)))
		}
	}
	return out
}

func renderPlan(text string, w int) []string {
	var out []string
	top := "── plan "
	out = append(out, stDim.Render(top+strings.Repeat("─", maxI(0, w-len(top)))))
	for _, l := range renderMarkdown(text, w-2) {
		out = append(out, "  "+l)
	}
	bot := "── (/approve to run) "
	out = append(out, stDim.Render(bot+strings.Repeat("─", maxI(0, w-len(bot)))))
	return out
}

func maxI(a, b int) int {
	if a > b {
		return a
	}
	return b
}

func (m tuiModel) renderPerm(it tItem, w int) []string {
	if it.resolved >= 0 {
		word := "yes"
		st := stDim
		switch it.resolved {
		case permAlways:
			word, st = "yes, always", stGreen
		case permNo:
			word, st = "no", stErr
		}
		return []string{stWarn.Render("▌") + stDim.Render(" "+it.text+" — ") + st.Render(word)}
	}
	radio := func(i int, label string) string {
		dot := "( )"
		st := stDim
		if m.permIdx == i {
			dot = "(●)"
			st = stSelB
		}
		return st.Render(dot + " " + label)
	}
	return []string{
		stWarn.Render("▌") + " " + stBold.Render("Allow "+it.text+"?"),
		stWarn.Render("▌") + "   " + radio(0, "Yes") + "   " +
			radio(1, "Yes, always") + "   " + radio(2, "No"),
		stWarn.Render("▌") + stDimmer.Render("   y yes · a always · n no · ↑↓/←→ select · enter"),
	}
}

// ---- banner ----------------------------------------------------------------

func (m tuiModel) renderBanner(w int) []string {
	// zdev HeaderBox: boxed galaxy on the left, "Welcome back" + facts on the right.
	wide := w >= 72
	cols, rows := 12, 4
	if wide {
		cols, rows = 24, 8
	}
	grid := logoGrid(cols, rows)
	title := " zagy " + tuiVersion + " "
	inner := maxI(20, w-2)
	topRule := maxI(2, inner-len(title)-1)
	top := stRose.Render("╭─") + stWhite.Render(title) + stRose.Render(strings.Repeat("─", topRule)+"╮")

	greet := "Welcome back."
	if u := strings.TrimSpace(m.user); u != "" {
		greet = "Welcome back, " + u + "."
	}
	facts := []string{
		stWhite.Render(greet),
		stDim.Render(truncANSI(m.agent.ModelAlias, maxI(8, w-cols-10))),
		stGhost.Render(truncANSI(tilde(m.cwd), maxI(8, w-cols-10))),
	}
	if wide {
		facts = append(facts, "",
			stDim.Render("/ commands · @ files · ! shell · # remember"),
			stDim.Render("shift+tab mode · ? shortcuts · esc interrupt"),
		)
	}
	if m.agent.Ctx.SandboxDegraded != "" {
		facts = append(facts, stWarn.Render("sandbox degraded: "+m.agent.Ctx.SandboxDegraded))
	}

	var out []string
	out = append(out, top)
	bodyH := maxI(len(grid), len(facts))
	for i := 0; i < bodyH; i++ {
		left := ""
		if i < len(grid) {
			left = logoRowStyled(grid[i])
		} else {
			left = strings.Repeat(" ", cols)
		}
		right := ""
		if i < len(facts) {
			right = facts[i]
		}
		row := stRose.Render("│ ") + left + "   " + truncANSI(right, maxI(8, w-cols-8))
		pad := w - lipgloss.Width(row) - 1
		if pad < 0 {
			pad = 0
		}
		row += strings.Repeat(" ", pad) + stRose.Render("│")
		out = append(out, row)
	}
	out = append(out, stRose.Render("╰"+strings.Repeat("─", maxI(2, w-2))+"╯"))
	return out
}

// ---- mini markdown ----------------------------------------------------------

// inlineMarkdown styles `code` and **bold** spans.
func inlineMarkdown(s string) string {
	var b strings.Builder
	bold, code := false, false
	for i := 0; i < len(s); {
		switch {
		case strings.HasPrefix(s[i:], "**") &&
			(bold || strings.Contains(s[i+2:], "**")):
			bold = !bold
			i += 2
		case s[i] == '`' &&
			(code || strings.IndexByte(s[i+1:], '`') >= 0):
			code = !code
			i++
		default:
			j := i
			for j < len(s) && !strings.HasPrefix(s[j:], "**") && s[j] != '`' {
				j++
			}
			seg := s[i:j]
			switch {
			case code:
				b.WriteString(stCode.Render(seg))
			case bold:
				b.WriteString(stBold.Render(seg))
			default:
				b.WriteString(seg)
			}
			i = j
		}
	}
	return b.String()
}

// wrapStyled wraps ANSI-styled text at w cells, indenting continuation lines.
func wrapStyled(s string, w int, indent string) []string {
	if w < 4 {
		w = 4
	}
	wrapped := ansi.Wordwrap(s, w-len(indent), "")
	var out []string
	for i, l := range strings.Split(wrapped, "\n") {
		if i == 0 {
			out = append(out, l)
		} else {
			out = append(out, indent+l)
		}
	}
	return out
}

func renderMarkdown(text string, w int) []string {
	if w < 8 {
		w = 8
	}
	var out []string
	inCode := false
	for _, line := range strings.Split(strings.ReplaceAll(text, "\r\n", "\n"), "\n") {
		t := strings.TrimSpace(line)
		if strings.HasPrefix(t, "```") {
			inCode = !inCode
			continue
		}
		if inCode {
			out = append(out, "  "+truncANSI(stCode.Render(line), w))
			continue
		}
		switch {
		case t == "":
			out = append(out, "")
		case strings.HasPrefix(t, "###"):
			out = append(out, wrapStyled(stBold.Render(strings.TrimSpace(t[3:])), w, " ")...)
		case strings.HasPrefix(t, "##"):
			out = append(out, wrapStyled(stBold.Render(strings.TrimSpace(t[2:])), w, " ")...)
		case strings.HasPrefix(t, "#"):
			out = append(out, wrapStyled(stBold.Render(strings.TrimSpace(t[1:])), w, " ")...)
		case strings.HasPrefix(t, "- ") || strings.HasPrefix(t, "* "):
			for i, l := range wrapStyled(inlineMarkdown(t[2:]), w-2, "") {
				if i == 0 {
					out = append(out, "• "+l)
				} else {
					out = append(out, "  "+l)
				}
			}
		case strings.HasPrefix(t, "• "):
			for i, l := range wrapStyled(inlineMarkdown(t[2:]), w-2, "") {
				if i == 0 {
					out = append(out, "• "+l)
				} else {
					out = append(out, "  "+l)
				}
			}
		default:
			out = append(out, wrapStyled(inlineMarkdown(t), w, " ")...)
		}
	}
	return out
}

// ---- slash dropdown ----------------------------------------------------------

func (m tuiModel) slashMenu() string {
	rows := m.slashRows()
	const maxRows = 8
	// scroll window follows the selection
	top := 0
	if m.slashSel >= maxRows {
		top = m.slashSel - maxRows + 1
	}
	show := rows[top:]
	more := 0
	if len(show) > maxRows {
		more = len(show) - maxRows
		show = show[:maxRows]
	}
	nameW := 0
	for _, r := range rows {
		if len(r.name) > nameW {
			nameW = len(r.name)
		}
	}
	var out []string
	if top > 0 {
		out = append(out, stDim.Render(fmt.Sprintf("   ↑ %d more", top)))
	}
	for i, r := range show {
		idx := top + i
		pad := strings.Repeat(" ", nameW-len(r.name)+2)
		var row string
		if idx == m.slashSel {
			row = stSelB.Render("> "+r.name) + pad + stSel.Render(r.desc)
		} else {
			row = "  " + stBase.Render(r.name) + pad + stDim.Render(r.desc)
		}
		out = append(out, truncANSI(row, m.width))
	}
	if more > 0 {
		out = append(out, stDim.Render(fmt.Sprintf("   ↓ %d more", more)))
	}
	if top == 0 && more == 0 {
		out = append(out, "") // keep height stable
	}
	return strings.Join(out, "\n")
}

// ---- help overlay ------------------------------------------------------------

var helpTabs = []string{"general", "commands", "shortcuts"}

func (m tuiModel) helpView() string {
	var b strings.Builder
	// tab bar
	b.WriteString(stSelB.Render("zagy"))
	b.WriteString("   ")
	for i, t := range helpTabs {
		if i == m.helpTab {
			st := stSel.Underline(true)
			b.WriteString(st.Render(t))
		} else {
			b.WriteString(stDim.Render(t))
		}
		b.WriteString("   ")
	}
	b.WriteString(stDimmer.Render("{</> or tab to cycle}"))
	b.WriteByte('\n')
	// title + note
	titles := []string{"General", "Commands", "Keyboard Shortcuts"}
	b.WriteString(stBold.Render(titles[m.helpTab]))
	b.WriteByte('\n')
	notes := []string{
		"session info",
		"type / in the composer to filter commands",
		"keybindings are fixed — permissions live in config.toml",
	}
	b.WriteString(stDim.Render(notes[m.helpTab]))
	b.WriteByte('\n')
	// rows with selection window
	rows := m.helpRows()
	page := m.helpPage()
	top := 0
	if m.helpSel >= page {
		top = m.helpSel - page + 1
	}
	show := rows[top:]
	if len(show) > page {
		show = show[:page]
	}
	leftW := 0
	for _, r := range rows {
		if w := lipgloss.Width(r.left); w > leftW {
			leftW = w
		}
	}
	for i, r := range show {
		idx := top + i
		pad := strings.Repeat(" ", leftW-lipgloss.Width(r.left)+3)
		var row string
		if idx == m.helpSel {
			row = stSelB.Render("> "+r.left) + pad + stSel.Render(r.right)
		} else {
			row = "  " + stDim.Render(r.left) + pad + stBase.Render(r.right)
		}
		b.WriteString(truncANSI(row, m.width))
		b.WriteByte('\n')
	}
	lo, hi := top+1, top+len(show)
	if len(rows) == 0 {
		lo, hi = 0, 0
	}
	b.WriteString(stDim.Render(fmt.Sprintf("[%d-%d of %d items]", lo, hi, len(rows))))
	return b.String()
}

// ---- model picker ------------------------------------------------------------

func (m tuiModel) modelView() string {
	var b strings.Builder
	b.WriteString(stBold.Render("Pick a model"))
	b.WriteByte('\n')
	for i, r := range m.models {
		var line strings.Builder
		if i == m.modelSel {
			line.WriteString(stSelB.Render("> " + r.alias))
		} else {
			line.WriteString("  " + r.alias)
		}
		detail := r.entry.Provider + "/" + r.entry.ID
		if r.entry.Context > 0 {
			detail += fmt.Sprintf(" · %s ctx", humanTok(uint64(r.entry.Context)))
		}
		free := true
		for _, p := range r.entry.Price {
			if p != 0 {
				free = false
			}
		}
		if free {
			detail += " · FREE"
		}
		line.WriteString("   " + stDim.Render(detail))
		if r.current {
			line.WriteString(" " + stGreen.Render("✳"))
		}
		b.WriteString(truncANSI(line.String(), m.width))
		b.WriteByte('\n')
	}
	return strings.TrimRight(b.String(), "\n")
}

func (m tuiModel) providersView() string {
	rows := filterProviders(m.provFilter)
	var b strings.Builder
	b.WriteString(stBold.Render("Providers"))
	b.WriteByte('\n')
	b.WriteString(stDim.Render("Keys stay on this machine and work in zcli and zdev too. Devin uses your subscription."))
	b.WriteByte('\n')
	b.WriteString(stDim.Render("/ "))
	if m.provFilter == "" {
		b.WriteString(stGhost.Render(fmt.Sprintf("Type to search %d providers", len(connectableProviders()))))
	} else {
		b.WriteString(m.provFilter)
	}
	b.WriteByte('\n')
	if len(rows) == 0 {
		b.WriteString(stDim.Render("  No providers match"))
		return b.String()
	}
	page := 12
	if m.provSel >= len(rows) {
		m.provSel = len(rows) - 1
	}
	top := m.provSel - (page - 1)
	if top < 0 {
		top = 0
	}
	for vi := top; vi < len(rows) && vi < top+page; vi++ {
		p := rows[vi]
		on := vi == m.provSel
		ready, detail := providerStatus(p)
		mark := "· "
		name := catalogDisplayName(p)
		if on {
			mark = "❯ "
			b.WriteString(stSelB.Render(mark + padRunes(name, 28)))
		} else {
			b.WriteString(mark + padRunes(name, 28))
		}
		if ready {
			b.WriteString("  " + stGreen.Render("✓ "+detail))
		} else {
			b.WriteString("  " + stDim.Render(detail))
		}
		b.WriteByte('\n')
	}
	if sel := m.provSel; sel >= 0 && sel < len(rows) {
		p := rows[sel]
		how := pitchFor(p)
		switch p.Kind {
		case "chatgpt":
			how = "Opens in zcli/zdev — use your ChatGPT Plus/Pro plan instead of an API key"
		case "devin":
			how = "Uses your Devin subscription models. Cosmos runs the tools — not Devin's agent."
		}
		b.WriteString(stDim.Render("  " + how))
	}
	return strings.TrimRight(b.String(), "\n")
}

func (m tuiModel) keyEntryView() string {
	p := catalogProvider(m.keyProvID)
	name := m.keyProvID
	env, url := "", ""
	if p != nil {
		name = p.Name
		if len(p.Env) > 0 {
			env = p.Env[0]
		}
		url = pinURL(p.ID)
	}
	var b strings.Builder
	b.WriteString(stBold.Render(name + " API key"))
	b.WriteByte('\n')
	b.WriteString(stDim.Render("Paste or type it — hidden as you type."))
	b.WriteByte('\n')
	b.WriteString(stPrompt.Render("❯ ") + strings.Repeat("•", minI(utf8.RuneCountInString(m.keyBuf), 48)))
	b.WriteByte('\n')
	if url != "" {
		b.WriteString(stDim.Render("get one at " + url))
		b.WriteByte('\n')
	}
	if env != "" {
		b.WriteString(stDim.Render("or set " + env + " in your environment"))
	}
	return strings.TrimRight(b.String(), "\n")
}

// ---- misc --------------------------------------------------------------------

func shortArgs(inp any) string {
	mm, _ := inp.(map[string]any)
	if mm == nil {
		return ""
	}
	var parts []string
	prefer := []string{"path", "cmd", "command", "query", "pattern", "url",
		"prompt", "text", "edits", "content", "plan", "capability"}
	for _, k := range prefer {
		v, ok := mm[k]
		if !ok {
			continue
		}
		switch vv := v.(type) {
		case string:
			s := vv
			if len(s) > 60 {
				s = s[:60] + "…"
			}
			parts = append(parts, fmt.Sprintf("%s=%q", k, s))
		case []any:
			parts = append(parts, fmt.Sprintf("%s=<%d>", k, len(vv)))
		default:
			b, _ := json.Marshal(v)
			s := string(b)
			if len(s) > 40 {
				s = s[:40] + "…"
			}
			parts = append(parts, k+"="+s)
		}
		if len(parts) == 3 {
			break
		}
	}
	if len(parts) == 0 {
		var ks []string
		for k := range mm {
			ks = append(ks, k)
		}
		sort.Strings(ks)
		for i, k := range ks {
			if i == 2 {
				break
			}
			b, _ := json.Marshal(mm[k])
			s := string(b)
			if len(s) > 40 {
				s = s[:40] + "…"
			}
			parts = append(parts, k+"="+s)
		}
	}
	return strings.Join(parts, " ")
}
