// welcome.go — first-run flow matching zcli-welcome / zdev welcome:
// a warp into the family spiral galaxy; the field dissolves and its dust
// rearranges into "welcome to cosmos"; then your name, BYOK keys (shared
// credentials.json), and a one-screen how-to. Owns the whole terminal
// while it runs (as a TUI overlay or `zagy onboarding`).
package main

import (
	"encoding/json"
	"fmt"
	"math"
	"os"
	"path/filepath"
	"strings"
	"time"
	"unicode/utf8"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
)

const (
	onboardingVersion = "2"
	welcomeSubtitle   = "zagy v0.1.0 · the cosmos coding agent"
	welcomeQuestion   = "what should i call you?"
	welcomeQuitCode   = 130
)

var welcomeSteps = []string{"name", "keys", "model", "start"}

type welcomeProv struct {
	id, name, env, url, kind, base string
}

func welcomeFromCatalog() []welcomeProv {
	var out []welcomeProv
	for _, p := range connectableProviders() {
		env := ""
		if len(p.Env) > 0 {
			env = p.Env[0]
		}
		out = append(out, welcomeProv{
			id: p.ID, name: catalogDisplayName(p), env: env,
			url: pinURL(p.ID), kind: p.Kind, base: p.BaseURL,
		})
	}
	return out
}

func onboardMarkerPath() string {
	return filepath.Join(cliHome(), ".zagy-onboarded")
}

func welcomeNeeded() bool {
	if os.Getenv("ZAGY_NO_ONBOARDING") != "" || os.Getenv("ZCLI_NO_ONBOARDING") != "" {
		return false
	}
	b, err := os.ReadFile(onboardMarkerPath())
	if err != nil {
		return true
	}
	return strings.TrimSpace(string(b)) != onboardingVersion
}

func markOnboarded() {
	_ = os.MkdirAll(cliHome(), 0o755)
	_ = os.WriteFile(onboardMarkerPath(), []byte(onboardingVersion+"\n"), 0o644)
}

func configUserName() string {
	if n := strings.TrimSpace(loadConfig(cliHome()).UserName); n != "" {
		return n
	}
	// family marker from zcli-welcome
	p := filepath.Join(cliHome(), "zcli-onboarded.json")
	raw, err := os.ReadFile(p)
	if err != nil {
		return ""
	}
	var v struct {
		Name string `json:"name"`
	}
	if json.Unmarshal(raw, &v) != nil {
		return ""
	}
	return strings.TrimSpace(v.Name)
}

// ---- star field (same warp / coalesce as zdev welcome) ---------------------

type rng64 struct{ s uint64 }

func (r *rng64) next() float64 {
	r.s ^= r.s << 13
	r.s ^= r.s >> 7
	r.s ^= r.s << 17
	return float64(r.s>>40) / 16777216.0
}

type wStar struct {
	x, y, z, v, l         float64
	px, py                float64
	ox, oy, tx, ty, delay float64
	dissolve              bool
}

type wTwinkle struct {
	x, y               int
	phase, speed, base float64
}

func starCount(w, h int) int {
	n := (w * h) / 9
	if n < 240 {
		n = 240
	}
	if n > 640 {
		n = 640
	}
	return n
}

func seedGalaxy(rng *rng64, n int) []wStar {
	out := make([]wStar, 0, n)
	for i := 0; i < n; i++ {
		core := i%8 == 0
		var r, th float64
		if core {
			r = 0.07 * math.Pow(rng.next(), 1.5)
			th = rng.next() * math.Pi * 2
		} else {
			r = 0.04 + 0.34*math.Pow(rng.next(), 0.9)
			th = float64(i%2)*math.Pi + r*9 + (rng.next()-0.5)*0.7
		}
		l := 0.7 + 0.9*rng.next()*(1-r)
		if core {
			l = 1.2 + 1.6*math.Pow(1-r/0.07, 2)
		}
		out = append(out, wStar{
			x: r * math.Cos(th), y: r * math.Sin(th) * 1.6,
			z: 0.92 + rng.next()*0.08, v: 0.9 + rng.next()*0.2, l: l,
		})
	}
	return out
}

func seedSky(w, h int) []wTwinkle {
	rng := rng64{s: 0x5851f42d4c957f2d ^ uint64(w*131+h)}
	n := (w * h) / 55
	if n < 12 {
		n = 12
	}
	out := make([]wTwinkle, n)
	for i := range out {
		out[i] = wTwinkle{
			x: int(rng.next() * float64(w)), y: int(rng.next() * float64(h)),
			phase: rng.next() * math.Pi * 2,
			speed: 0.6 + rng.next()*1.8,
			base:  0.3 + rng.next()*0.7,
		}
	}
	return out
}

func smooth(t float64) float64 {
	if t < 0 {
		return 0
	}
	if t > 1 {
		return 1
	}
	return t * t * (3 - 2*t)
}

func starGlyph(b float64) (string, string, bool) {
	switch {
	case b > 0.85:
		return "✶", "#f4f4f4", true
	case b > 0.6:
		return "✦", "#d8d8d8", false
	case b > 0.35:
		return "•", "#a8a8a8", false
	default:
		return "·", "#666666", false
	}
}

func grayHex(v int) string {
	if v < 0 {
		v = 0
	}
	if v > 255 {
		v = 255
	}
	return fmt.Sprintf("#%02x%02x%02x", v, v, v)
}

// ---- canvas ----------------------------------------------------------------

type wCell struct {
	ch   string
	fg   string
	bold bool
	z    float64
}

type wCanvas struct {
	w, h  int
	cells []wCell
}

func newCanvas(w, h int) *wCanvas {
	if w < 8 {
		w = 8
	}
	if h < 8 {
		h = 8
	}
	return &wCanvas{w: w, h: h, cells: make([]wCell, w*h)}
}

func (c *wCanvas) idx(x, y int) int { return y*c.w + x }

func (c *wCanvas) set(x, y int, ch, fg string, bold bool, z float64) {
	if x < 0 || y < 0 || x >= c.w || y >= c.h {
		return
	}
	i := c.idx(x, y)
	if c.cells[i].ch != "" && c.cells[i].z >= z {
		return
	}
	c.cells[i] = wCell{ch: ch, fg: fg, bold: bold, z: z}
}

func (c *wCanvas) text(x, y int, s, fg string, bold bool) {
	for _, r := range s {
		if x >= c.w {
			break
		}
		c.set(x, y, string(r), fg, bold, 10)
		x++
	}
}

func (c *wCanvas) center(y int, s, fg string, bold bool) {
	x := (c.w - utf8.RuneCountInString(s)) / 2
	c.text(x, y, s, fg, bold)
}

func (c *wCanvas) clear(x, y, w, h int) {
	for yy := y; yy < y+h && yy < c.h; yy++ {
		for xx := x; xx < x+w && xx < c.w; xx++ {
			if xx >= 0 && yy >= 0 {
				c.cells[c.idx(xx, yy)] = wCell{}
			}
		}
	}
}

func (c *wCanvas) render() string {
	var b strings.Builder
	for y := 0; y < c.h; y++ {
		if y > 0 {
			b.WriteByte('\n')
		}
		run := ""
		fg, bold := "", false
		flush := func() {
			if run == "" {
				return
			}
			st := lipgloss.NewStyle()
			if fg != "" {
				st = st.Foreground(lipgloss.Color(fg))
			}
			if bold {
				st = st.Bold(true)
			}
			b.WriteString(st.Render(run))
			run = ""
		}
		for x := 0; x < c.w; x++ {
			cell := c.cells[c.idx(x, y)]
			ch := cell.ch
			if ch == "" {
				ch = " "
			}
			if cell.fg != fg || cell.bold != bold {
				flush()
				fg, bold = cell.fg, cell.bold
			}
			run += ch
		}
		flush()
	}
	return b.String()
}

// ---- flow ------------------------------------------------------------------

type welcomePhase int

const (
	wWarp welcomePhase = iota
	wCoalesce
	wName
	wKeys
	wKey
	wReady
)

type welcomeTickMsg time.Time

type welcomeFlow struct {
	phase           welcomePhase
	w, h            int
	t0, clock, last time.Time
	rot             float64
	rng             rng64
	stars           []wStar
	sky             []wTwinkle
	art             titleArt
	gx, gy          int

	name     string
	filter   string
	sel      int
	keyIdx   int
	keyBuf   string
	keyed    map[string]bool
	status   string
	statusOK bool
	statusAt time.Time
	model    string
	provs    []welcomeProv
	done     bool
	quit     bool
}

func newWelcome(w, h int) *welcomeFlow {
	if w < 40 {
		w = 80
	}
	if h < 12 {
		h = 24
	}
	f := &welcomeFlow{
		phase:  wWarp,
		w:      w,
		h:      h,
		t0:     time.Now(),
		clock:  time.Now(),
		last:   time.Now(),
		rng:    rng64{s: 0x9e3779b97f4a7c15},
		keyed:  map[string]bool{},
		keyIdx: -1,
		provs:  welcomeFromCatalog(),
		name:   configUserName(),
	}
	f.stars = seedGalaxy(&f.rng, starCount(w, h))
	f.sky = seedSky(w, h)
	f.layoutTitle()
	f.refreshKeyed()
	return f
}

func (f *welcomeFlow) layoutTitle() {
	f.art = makeTitleArt(f.w-4, f.h-14)
	f.gx = maxI(0, (f.w-f.art.cols)/2)
	f.gy = maxI(0, (f.h-(f.art.rows+9))/2)
}

func (f *welcomeFlow) resize(w, h int) {
	if w == f.w && h == f.h {
		return
	}
	f.w, f.h = w, h
	f.sky = seedSky(w, h)
	f.layoutTitle()
	if f.phase == wCoalesce {
		f.beginCoalesce()
	}
}

func (f *welcomeFlow) refreshKeyed() {
	f.keyed = map[string]bool{}
	for _, p := range f.provs {
		switch p.kind {
		case "devin":
			if devinKnownReady() {
				f.keyed[p.id] = true
			}
		case "chatgpt":
			// ChatGPT OAuth lives in zcli/zdev
		default:
			if (p.env != "" && os.Getenv(p.env) != "") || credHas(p.id) {
				f.keyed[p.id] = true
			}
		}
	}
}

func (f *welcomeFlow) t() float64    { return time.Since(f.t0).Seconds() }
func (f *welcomeFlow) secs() float64 { return time.Since(f.clock).Seconds() }

func (f *welcomeFlow) flash(s string, ok bool) {
	f.status, f.statusOK, f.statusAt = s, ok, time.Now()
}

func (f *welcomeFlow) schedule() tea.Cmd {
	return tea.Tick(16*time.Millisecond, func(t time.Time) tea.Msg { return welcomeTickMsg(t) })
}

func (f *welcomeFlow) step(now time.Time) {
	if f.last.IsZero() {
		f.last = now
	}
	dt := now.Sub(f.last).Seconds()
	if dt < 0 {
		dt = 0
	}
	if dt > 0.05 {
		dt = 0.05
	}
	f.last = now
	switch f.phase {
	case wWarp:
		f.advanceWarp(dt)
	case wCoalesce:
		if f.t() > 2.0 {
			f.beginName()
		}
	}
}

func (f *welcomeFlow) advanceWarp(dt float64) {
	t := f.t()
	if t > 2.9 {
		f.beginCoalesce()
		return
	}
	u := smooth(t / 2.6)
	speed := 0.04 + 1.7*u*u
	f.rot += dt * (0.35 + 0.8*u)
	sn, cs := math.Sin(f.rot), math.Cos(f.rot)
	cx := float64(f.w) / 2
	cy := float64(f.h) / 2
	sy := math.Min(cy, cx*0.45)
	for i := range f.stars {
		s := &f.stars[i]
		s.z -= speed * dt * s.v
		if s.z < 0.07 {
			s.x = f.rng.next()*2 - 1
			s.y = f.rng.next()*1.1 - 0.55
			s.z = 0.55 + f.rng.next()*0.45
			s.l = 1
		}
		rx := s.x*cs - s.y*sn
		ry := s.x*sn + s.y*cs
		s.px = cx + (rx/s.z)*cx*0.92
		s.py = cy + (ry/s.z)*sy*0.92
	}
}

func (f *welcomeFlow) beginCoalesce() {
	cx := float64(f.w) / 2
	cy := float64(f.h) / 2
	n := len(f.art.cells)
	targets := n
	if t := (len(f.stars) * 3) / 4; t > targets {
		targets = t
	}
	for i := range f.stars {
		s := &f.stars[i]
		on := s.px >= 0 && s.px < float64(f.w) && s.py >= 0 && s.py < float64(f.h)
		if on {
			s.ox, s.oy = s.px, s.py
		} else {
			edge := int(f.rng.next() * 4)
			switch edge {
			case 0:
				s.ox, s.oy = 0, f.rng.next()*float64(f.h)
			case 1:
				s.ox, s.oy = float64(f.w-1), f.rng.next()*float64(f.h)
			case 2:
				s.ox, s.oy = f.rng.next()*float64(f.w), 0
			default:
				s.ox, s.oy = f.rng.next()*float64(f.w), float64(f.h-1)
			}
		}
		s.delay = f.rng.next() * 0.5
		if n > 0 && i < targets {
			cell := f.art.cells[(i*7919)%n]
			s.tx = float64(f.gx+cell[0]) + 0.5
			s.ty = float64(f.gy+cell[1]) + 0.5
			s.dissolve = false
		} else {
			s.dissolve = true
			s.tx = s.ox + (s.ox-cx)*0.6
			s.ty = s.oy + (s.oy-cy)*0.6
		}
	}
	f.phase = wCoalesce
	f.t0 = time.Now()
}

func (f *welcomeFlow) beginName() {
	f.phase = wName
	f.t0 = time.Now()
}

func (f *welcomeFlow) skipIntro() {
	if f.phase == wWarp || f.phase == wCoalesce {
		f.beginName()
	}
}

func (f *welcomeFlow) finish() {
	name := strings.TrimSpace(f.name)
	if name != "" {
		_ = setUserName(cliHome(), name)
	}
	if f.model != "" {
		_ = setDefaultModel(cliHome(), f.model)
	}
	markOnboarded()
	f.done = true
}

func (f *welcomeFlow) saveKey() {
	key := strings.TrimSpace(f.keyBuf)
	f.keyBuf = ""
	if f.keyIdx < 0 || f.keyIdx >= len(f.provs) {
		f.phase = wKeys
		return
	}
	p := f.provs[f.keyIdx]
	if key == "" {
		f.phase = wKeys
		return
	}
	if err := storeKey(p.id, key); err != nil {
		f.flash("✗ "+p.name+": "+err.Error(), false)
		f.phase = wKeys
		return
	}
	_ = ensureProvider(cliHome(), p)
	if loadConfig(cliHome()).DefaultModel == "" || loadConfig(cliHome()).DefaultModel == "fixture" {
		alias := p.id + "-auto"
		_ = ensureModel(cliHome(), p, "auto")
		_ = setDefaultModel(cliHome(), alias)
		f.model = alias
	}
	f.keyed[p.id] = true
	f.flash("✓ "+p.name+" key saved to credentials.json — works in zcli and zdev too", true)
	f.phase = wKeys
}

func (f *welcomeFlow) connectDevin() {
	devinInvalidate()
	if !devinReady() {
		f.flash(devinHint(), false)
		return
	}
	f.keyed["devin"] = true
	f.flash("✓ Devin · "+devinDetail(), true)
	def := catalogProvider("devin")
	if def == nil {
		return
	}
	models, err := liveFetch(*def)
	if err != nil {
		f.flash(err.Error(), false)
		return
	}
	liveStore("devin", models)
	if (f.model == "" || f.model == "fixture") && len(models) > 0 {
		alias := "devin/" + models[0].ID
		_ = setDefaultModel(cliHome(), alias)
		f.model = alias
	}
}

func (f *welcomeFlow) filteredProvs() []int {
	q := strings.ToLower(f.filter)
	var out []int
	for i, p := range f.provs {
		hay := strings.ToLower(p.id + " " + p.name + " " + p.env + " " + p.kind)
		if q == "" || strings.Contains(hay, q) {
			out = append(out, i)
		}
	}
	return out
}

func (f *welcomeFlow) onKey(msg tea.KeyMsg) tea.Cmd {
	key := msg.String()
	if key == "ctrl+c" || key == "ctrl+d" {
		f.quit = true
		f.done = true
		return nil
	}
	switch f.phase {
	case wWarp, wCoalesce:
		f.skipIntro()
		return nil
	case wName:
		switch key {
		case "enter":
			f.phase = wKeys
			f.filter = ""
			f.sel = 0
		case "esc":
			f.phase = wKeys
		case "backspace":
			if f.name != "" {
				_, sz := utf8.DecodeLastRuneInString(f.name)
				f.name = f.name[:len(f.name)-sz]
			}
		default:
			if msg.Type == tea.KeyRunes {
				for _, r := range msg.Runes {
					if r >= 32 {
						f.name += string(r)
					}
				}
			}
		}
	case wKeys:
		order := f.filteredProvs()
		items := len(order) + 1
		switch key {
		case "up", "k":
			if f.sel > 0 {
				f.sel--
			}
		case "down", "j":
			if f.sel < items-1 {
				f.sel++
			}
		case "enter":
			if f.sel >= len(order) {
				f.phase = wReady
			} else if f.sel >= 0 && f.sel < len(order) {
				f.keyIdx = order[f.sel]
				p := f.provs[f.keyIdx]
				if p.kind == "chatgpt" {
					f.flash("ChatGPT OAuth is in zcli and zdev — pick Devin or paste an API key here.", false)
				} else if p.kind == "devin" {
					f.connectDevin()
				} else {
					f.keyBuf = ""
					f.phase = wKey
				}
			}
		case "esc":
			f.phase = wReady
		case "backspace":
			if f.filter != "" {
				_, sz := utf8.DecodeLastRuneInString(f.filter)
				f.filter = f.filter[:len(f.filter)-sz]
			}
		default:
			if msg.Type == tea.KeyRunes {
				f.filter += string(msg.Runes)
				f.sel = 0
			}
		}
	case wKey:
		switch key {
		case "enter":
			f.saveKey()
		case "esc":
			f.keyBuf = ""
			f.phase = wKeys
		case "backspace":
			if f.keyBuf != "" {
				_, sz := utf8.DecodeLastRuneInString(f.keyBuf)
				f.keyBuf = f.keyBuf[:len(f.keyBuf)-sz]
			}
		default:
			if msg.Type == tea.KeyRunes {
				f.keyBuf += string(msg.Runes)
			}
		}
	case wReady:
		switch key {
		case "enter":
			f.finish()
		case "esc":
			f.phase = wKeys
		}
	}
	return nil
}

func (f *welcomeFlow) View() string {
	c := newCanvas(f.w, f.h)
	switch f.phase {
	case wWarp:
		f.drawWarp(c)
	case wCoalesce:
		f.drawCoalesce(c)
	case wName:
		f.drawName(c)
	case wKeys, wKey:
		f.drawKeys(c)
	case wReady:
		f.drawReady(c)
	}
	return c.render()
}

func (f *welcomeFlow) drawWarp(c *wCanvas) {
	t := f.t()
	sn, cs := math.Sin(f.rot), math.Cos(f.rot)
	cx := float64(f.w) / 2
	cy := float64(f.h) / 2
	sy := math.Min(cy, cx*0.45)
	u := smooth(t / 2.6)
	speed := 0.04 + 1.7*u*u
	for i := range f.stars {
		s := &f.stars[i]
		proj := func(z float64) (float64, float64) {
			rx := s.x*cs - s.y*sn
			ry := s.x*sn + s.y*cs
			return cx + (rx/z)*cx*0.92, cy + (ry/z)*sy*0.92
		}
		hx, hy := proj(s.z)
		s.px, s.py = hx, hy
		tx, ty := proj(s.z + speed*0.05*s.v*3.5)
		b := math.Min(1, (0.32*s.l)/s.z)
		n := int(math.Round(math.Hypot(hx-tx, hy-ty)))
		if n < 1 {
			n = 1
		}
		if n > 5 {
			n = 5
		}
		for k := 0; k <= n; k++ {
			frac := float64(k) / float64(n)
			bk := b * (1 - frac*0.7)
			ch, fg, bold := starGlyph(bk)
			if k == 0 {
				ch, fg, bold = starGlyph(b)
			}
			c.set(int(hx+(tx-hx)*frac), int(hy+(ty-hy)*frac), ch, fg, bold, bk)
		}
	}
	if t > 0.6 {
		c.text(f.w-len("any key to skip")-2, f.h-2, "any key to skip", "#6c6c6c", false)
	}
}

func (f *welcomeFlow) drawCoalesce(c *wCanvas) {
	t := f.t()
	for i := range f.stars {
		s := &f.stars[i]
		e := smooth((t - s.delay) / 0.9)
		x := s.ox + (s.tx-s.ox)*e
		y := s.oy + (s.ty-s.oy)*e
		s.px, s.py = x, y
		b, ch := 0.35+0.65*e, "·"
		if e > 0.75 {
			ch = "✦"
		}
		if s.dissolve {
			b, ch = 0.5*(1-e), "·"
		}
		if b > 0.03 {
			_, fg, bold := starGlyph(b)
			c.set(int(x), int(y), ch, fg, bold, b)
		}
	}
	if t > 1.0 {
		reveal := math.Min(1, (t-1.0)/0.5)
		span := 0
		for _, cell := range f.art.cells {
			if cell[0] > span {
				span = cell[0]
			}
		}
		span++
		for _, cell := range f.art.cells {
			if cell[0] < int(float64(span)*reveal) {
				c.set(f.gx+cell[0], f.gy+cell[1], string(rune(cell[2])), "#f4f4f4", true, 8)
			}
		}
	}
	if t > 1.3 {
		n := int((t - 1.3) / 0.012)
		if n > len(welcomeSubtitle) {
			n = len(welcomeSubtitle)
		}
		sub := welcomeSubtitle[:n]
		c.center(f.gy+f.art.rows+1, sub, "#8a8a8a", false)
	}
}

func (f *welcomeFlow) drawSky(c *wCanvas) {
	t := f.secs()
	for _, s := range f.sky {
		b := s.base * (0.5 + 0.5*math.Sin(t*s.speed+s.phase))
		ch, v := "·", 0x36
		if b > 0.85 {
			ch, v = "✦", 0x70
		} else if b > 0.45 {
			v = 0x5a
		}
		c.set(s.x, s.y, ch, grayHex(v), false, 0.1)
	}
}

func (f *welcomeFlow) drawHeader(c *wCanvas, step int) {
	c.clear(0, 0, c.w, 3)
	c.text(3, 1, "✶ ", "#5cd0ff", false)
	c.text(5, 1, "cosmos", "", true)
	x := c.w - 3
	for i := len(welcomeSteps) - 1; i >= 0; i-- {
		s := welcomeSteps[i]
		label := s
		fg, bold := "#8a8a8a", false
		if i < step {
			label = "✓ " + s
			fg = "#5fd787"
		} else if i == step {
			fg, bold = "#5cd0ff", true
		}
		x -= utf8.RuneCountInString(label)
		c.text(x, 1, label, fg, bold)
		if i > 0 {
			x -= 3
			c.text(x, 1, " ─ ", "#4a4a4a", false)
		}
	}
}

func (f *welcomeFlow) drawFooter(c *wCanvas, hint string) {
	c.clear(0, c.h-2, c.w, 1)
	c.center(c.h-2, hint, "#8a8a8a", false)
}

func (f *welcomeFlow) drawTitle(c *wCanvas) {
	span := 1
	for _, cell := range f.art.cells {
		if cell[0] > span {
			span = cell[0]
		}
	}
	pos := math.Mod(f.secs()*0.35, 1)*(float64(span)+40) - 20
	c.clear(f.gx-2, f.gy-1, span+5, f.art.rows+3)
	for _, cell := range f.art.cells {
		d := (float64(cell[0]) - pos) / 6
		v := 200 + 55*math.Exp(-d*d)
		c.set(f.gx+cell[0], f.gy+cell[1], string(rune(cell[2])), grayHex(int(v)), true, 8)
	}
	c.center(f.gy+f.art.rows+1, welcomeSubtitle, "#8a8a8a", false)
}

func (f *welcomeFlow) drawName(c *wCanvas) {
	f.drawSky(c)
	f.drawTitle(c)
	y := f.gy + f.art.rows + 4
	bw := 34
	if bw > f.w-4 {
		bw = f.w - 4
	}
	x := (f.w - bw) / 2
	c.clear(x, y, bw, 4)
	c.center(y, welcomeQuestion, "", true)
	c.text(x, y+2, "❯ ", "#5cd0ff", true)
	c.text(x+2, y+2, f.name, "#f4f4f4", false)
	c.text(x, y+3, strings.Repeat("─", bw), "#4a4a4a", false)
	f.drawFooter(c, "enter to continue · esc to skip")
}

func (f *welcomeFlow) drawKeys(c *wCanvas) {
	f.drawSky(c)
	f.drawHeader(c, 1)
	cw := f.w - 8
	if cw > 100 {
		cw = 100
	}
	if cw < 20 {
		cw = 20
	}
	x := (f.w - cw) / 2
	top := 4
	c.clear(x-2, top, cw+4, c.h-top-3)
	c.text(x, top, "Bring your own keys", "", true)
	if len(f.keyed) > 0 {
		s := fmt.Sprintf("%d connected", len(f.keyed))
		c.text(x+cw-len(s), top, s, "#5fd787", false)
	}
	c.text(x, top+1, truncRunes("Keys stay on this machine (Windows Credential Manager, else credentials.json) and work in zcli and zdev too.", cw), "#8a8a8a", false)

	if f.phase == wKey && f.keyIdx >= 0 && f.keyIdx < len(f.provs) {
		p := f.provs[f.keyIdx]
		y := top + 4
		c.text(x, y, p.name+" API key", "", true)
		c.text(x, y+1, "Paste or type it — it's hidden as you type.", "#8a8a8a", false)
		c.text(x, y+3, "❯ ", "#5cd0ff", true)
		dots := strings.Repeat("•", minI(utf8.RuneCountInString(f.keyBuf), cw-4))
		c.text(x+2, y+3, dots, "#f4f4f4", false)
		c.text(x, y+4, strings.Repeat("─", minI(cw, 64)), "#4a4a4a", false)
		yy := y + 6
		if p.url != "" {
			c.text(x, yy, "get one at "+p.url, "#5cd0ff", false)
			yy++
		}
		c.text(x, yy, "or set "+p.env+" in your environment", "#8a8a8a", false)
		f.drawStatus(c, x, yy+2, cw)
		f.drawFooter(c, "enter save · esc back")
		return
	}

	fy := top + 3
	c.text(x, fy, "/ ", "#8a8a8a", false)
	if f.filter != "" {
		c.text(x+2, fy, f.filter, "#f4f4f4", false)
	} else {
		c.text(x+2, fy, fmt.Sprintf("Type to search %d providers", len(f.provs)), "#6c6c6c", false)
	}
	c.text(x, fy+1, strings.Repeat("─", cw), "#4a4a4a", false)
	order := f.filteredProvs()
	items := len(order) + 1
	if f.sel >= items {
		f.sel = items - 1
	}
	listY := fy + 2
	rows := c.h - listY - 6
	if rows < 3 {
		rows = 3
	}
	first := 0
	if f.sel-rows+1 > 0 {
		first = f.sel - rows + 1
	}
	nameW := 16
	for _, p := range f.provs {
		if n := utf8.RuneCountInString(p.name); n+2 > nameW {
			nameW = n + 2
		}
	}
	if nameW > 30 {
		nameW = 30
	}
	for r, vi := 0, first; vi < items && r < rows; vi, r = vi+1, r+1 {
		y := listY + r
		on := vi == f.sel
		mark, mfg, mbold := "  ", "#5cd0ff", false
		if on {
			mark, mbold = "❯ ", true
		}
		c.text(x, y, mark, mfg, mbold)
		if vi == len(order) {
			label := "Skip for now →"
			if f.model != "" || len(f.keyed) > 0 {
				label = "Continue →"
			}
			fg := "#8a8a8a"
			if on {
				fg = "#5cd0ff"
			}
			c.text(x+2, y, label, fg, on)
			continue
		}
		p := f.provs[order[vi]]
		fg := "#f4f4f4"
		if on {
			fg = "#5cd0ff"
		}
		c.text(x+2, y, truncRunes(p.name, nameW-2), fg, on)
		right := ""
		pitch := p.env
		switch p.kind {
		case "devin":
			pitch = "subscription via local CLI"
			if f.keyed[p.id] {
				right = "✓ " + truncRunes(devinDetail(), 22)
			}
		case "chatgpt":
			pitch = "use zcli / zdev to sign in"
		default:
			if f.keyed[p.id] {
				right = "✓ connected"
			}
		}
		c.text(x+2+nameW, y, truncRunes(pitch, cw-2-nameW-len(right)-2), "#8a8a8a", false)
		if right != "" {
			c.text(x+cw-len(right), y, right, "#5fd787", false)
		}
	}
	by := listY + rows
	c.text(x, by, strings.Repeat("─", cw), "#4a4a4a", false)
	f.drawStatus(c, x, by+1, cw)
	f.drawFooter(c, "↑↓ choose · enter connect · type to search · esc continue")
}

func (f *welcomeFlow) drawReady(c *wCanvas) {
	rows := [][2]string{
		{"type a task, enter", "zagy reads, edits and runs code with you"},
		{"/", "commands — /model /providers /onboarding /usage /compact /help"},
		{"@path", "attach a file · !cmd runs a shell command"},
		{"shift+tab", "cycle plan → agent → accept-edits → yolo"},
		{"esc", "interrupt the agent · ctrl+c twice quits"},
		{"?", "keyboard shortcuts"},
		{`zagy -p "…"`, "headless runs for scripts and CI"},
	}
	f.drawSky(c)
	f.drawHeader(c, 3)
	name := strings.TrimSpace(f.name)
	kw, dw := 0, 0
	for _, r := range rows {
		if n := utf8.RuneCountInString(r[0]); n > kw {
			kw = n
		}
		if n := utf8.RuneCountInString(r[1]); n > dw {
			dw = n
		}
	}
	bw := kw + 3 + dw
	if bw < 60 {
		bw = 60
	}
	if bw > f.w-4 {
		bw = f.w - 4
	}
	bh := len(rows) + 7
	x := (f.w - bw) / 2
	y := (f.h - bh) / 2
	if y < 0 {
		y = 0
	}
	c.clear(x-2, y-1, bw+4, bh+2)
	greet := "You're all set."
	if name != "" {
		greet = "You're all set, " + name + "."
	}
	c.text(x, y, "✶ ", "#5cd0ff", false)
	c.text(x+2, y, greet, "", true)
	if f.model != "" {
		c.text(x, y+2, "model  ", "#8a8a8a", false)
		c.text(x+7, y+2, f.model, "", true)
		c.text(x+7+len(f.model), y+2, "  · /model to switch", "#8a8a8a", false)
	} else {
		c.text(x, y+2, "no provider yet — /providers or /onboarding when you're ready", "#8a8a8a", false)
	}
	var keys []string
	for _, p := range f.provs {
		if f.keyed[p.id] {
			keys = append(keys, p.name)
		}
	}
	if len(keys) > 0 {
		c.text(x, y+3, "keys   ", "#8a8a8a", false)
		c.text(x+7, y+3, truncRunes(strings.Join(keys, " · "), bw-7), "#5fd787", false)
	}
	for i, r := range rows {
		c.text(x, y+5+i, padRunes(r[0], kw), "#5cd0ff", false)
		c.text(x+kw+3, y+5+i, r[1], "#8a8a8a", false)
	}
	c.text(x, y+6+len(rows), "enter ", "#5cd0ff", true)
	c.text(x+6, y+6+len(rows), "launch zagy", "#f4f4f4", false)
	f.drawFooter(c, "enter launch · esc back to keys")
}

func (f *welcomeFlow) drawStatus(c *wCanvas, x, y, w int) {
	if f.status != "" && time.Since(f.statusAt) < 8*time.Second {
		fg := "#ff6b6b"
		if f.statusOK {
			fg = "#5fd787"
		}
		c.text(x, y, truncRunes(f.status, w), fg, false)
	}
}

func truncRunes(s string, w int) string {
	if w <= 0 {
		return ""
	}
	n := utf8.RuneCountInString(s)
	if n <= w {
		return s
	}
	if w == 1 {
		return "…"
	}
	var b strings.Builder
	i := 0
	for _, r := range s {
		if i >= w-1 {
			break
		}
		b.WriteRune(r)
		i++
	}
	b.WriteRune('…')
	return b.String()
}

func padRunes(s string, w int) string {
	n := utf8.RuneCountInString(s)
	if n >= w {
		return s
	}
	return s + strings.Repeat(" ", w-n)
}

func minI(a, b int) int {
	if a < b {
		return a
	}
	return b
}

// standalone bubbletea wrapper for `zagy onboarding`
type welcomeProg struct{ f *welcomeFlow }

func (w welcomeProg) Init() tea.Cmd { return w.f.schedule() }
func (w welcomeProg) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch m := msg.(type) {
	case tea.WindowSizeMsg:
		w.f.resize(m.Width, m.Height)
		return w, nil
	case welcomeTickMsg:
		w.f.step(time.Time(m))
		if w.f.done {
			return w, tea.Quit
		}
		return w, w.f.schedule()
	case tea.KeyMsg:
		cmd := w.f.onKey(m)
		if w.f.done {
			return w, tea.Quit
		}
		return w, cmd
	}
	return w, nil
}
func (w welcomeProg) View() string { return w.f.View() }

func runOnboarding() int {
	if !stdinTTY() {
		fmt.Fprintln(os.Stderr, "zagy onboarding needs a terminal — run `zagy` in one")
		return ExitFailed
	}
	f := newWelcome(80, 24)
	p := tea.NewProgram(welcomeProg{f: f}, tea.WithAltScreen())
	if _, err := p.Run(); err != nil {
		fmt.Fprintln(os.Stderr, "welcome:", err)
		return ExitFailed
	}
	if f.quit {
		return welcomeQuitCode
	}
	return ExitDone
}
