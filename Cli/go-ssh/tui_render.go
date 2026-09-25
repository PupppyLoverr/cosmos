// tui_render.go — zagy TUI rendering: banner, transcript items, mini-markdown,
// slash dropdown, help/model overlays, permission card, footer.
package main

import (
	"encoding/json"
	"fmt"
	"sort"
	"strings"
	"time"

	"github.com/charmbracelet/lipgloss"
	"github.com/charmbracelet/x/ansi"
)

// palette — Antigravity-ish: cyan accents, gray dim text, red errors.
var (
	stBase   = lipgloss.NewStyle()
	stPrompt = lipgloss.NewStyle().Foreground(lipgloss.Color("14")).Bold(true)
	stSel    = lipgloss.NewStyle().Foreground(lipgloss.Color("14"))
	stSelB   = lipgloss.NewStyle().Foreground(lipgloss.Color("14")).Bold(true)
	stDim    = lipgloss.NewStyle().Foreground(lipgloss.Color("8"))
	stDimmer = lipgloss.NewStyle().Foreground(lipgloss.Color("240"))
	stBold   = lipgloss.NewStyle().Bold(true)
	stErr    = lipgloss.NewStyle().Foreground(lipgloss.Color("9"))
	stWarn   = lipgloss.NewStyle().Foreground(lipgloss.Color("11"))
	stGreen  = lipgloss.NewStyle().Foreground(lipgloss.Color("10"))
	stCode   = lipgloss.NewStyle().Foreground(lipgloss.Color("215"))
	stKey    = lipgloss.NewStyle().Foreground(lipgloss.Color("14")).Bold(true)
)

func rule(w int) string {
	if w < 1 {
		w = 1
	}
	return stDimmer.Render(strings.Repeat("─", w))
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
	case m.slashVisible():
		return stKey.Render("↑/↓") + stDim.Render(" Navigate · ") +
			stKey.Render("enter") + stDim.Render(" Select · ") +
			stKey.Render("tab") + stDim.Render(" Complete · ") +
			stKey.Render("esc") + stDim.Render(" to cancel")
	default:
		left := stDim.Render("? for shortcuts")
		right := stDim.Render(m.modelLabel() + " · " + m.agent.Mode)
		if m.totIn+m.totOut > 0 {
			right = stDim.Render(m.modelLabel()+" · "+m.agent.Mode+" · ") +
				stDimmer.Render(fmt.Sprintf("↑%s ↓%s", humanTok(m.totIn), humanTok(m.totOut)))
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

var logoArt = []struct{ art, color string }{
	{"    ██    ", "#FFD75F"},
	{"   ████   ", "#FF9E64"},
	{"  ██  ██  ", "#FF7A93"},
	{" ██    ██ ", "#9ECE6A"},
	{" ████████ ", "#7DCFFF"},
	{"███    ███", "#7AA2F7"},
}

func (m tuiModel) renderBanner(w int) []string {
	a := m.agent
	info := []string{
		stSelB.Render("zagy") + " " + tuiVersion,
		a.ModelAlias,
		stDim.Render(a.Mode + " · session " + a.Session.ID[:8]),
		stDim.Render(tilde(m.cwd)),
	}
	if a.Ctx.SandboxDegraded != "" {
		info = append(info, stWarn.Render("sandbox degraded: "+a.Ctx.SandboxDegraded))
	}
	var out []string
	for i, l := range logoArt {
		row := lipgloss.NewStyle().Foreground(lipgloss.Color(l.color)).Render(l.art)
		if i < len(info) {
			row += "   " + truncANSI(info[i], w-14)
		}
		out = append(out, row)
	}
	out = append(out, rule(w))
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
